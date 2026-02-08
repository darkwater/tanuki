#![feature(macro_attr)]

use core::{convert::Infallible, marker::PhantomData, str::FromStr as _, sync::atomic::AtomicU16};
use std::{collections::BTreeMap, sync::Arc};

use mqtt_endpoint_tokio::mqtt_ep::{
    self, Endpoint,
    packet::v5_0,
    role,
    transport::{TcpTransport, connect_helper},
};
use mqtt_protocol_core::mqtt::packet::{
    Property, Qos, SubEntry, SubOpts, SubscriptionIdentifier,
    v5_0::{Connack, Publish},
};
use serde::Serialize;
use tanuki_common::{
    EntityId, TanukiString, ToTanukiString, Topic,
    meta::{self, MetaField},
};
use tokio::sync::Mutex;

use self::capabilities::{Authority, EntityRole, User};
use crate::capabilities::{Capability, TanukiCapability};

pub mod capabilities;
pub mod log;
pub mod registry;

pub use tanuki_common as common;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("mqtt transport error: {0}")]
    MqttTransport(#[from] mqtt_ep::TransportError),
    #[error("mqtt connection error: {0}")]
    MqttConnection(#[from] mqtt_ep::ConnectionError),
    #[error("mqtt packet field error: {0}")]
    MqttPacketField(&'static str),
    #[error("mqtt packet error: {0}")]
    MqttPacket(mqtt_ep::result_code::MqttError),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("bad topic: {0}")]
    BadTopic(&'static str),
}

impl From<mqtt_ep::result_code::MqttError> for Error {
    fn from(e: mqtt_ep::result_code::MqttError) -> Self {
        Error::MqttPacket(e)
    }
}

pub(crate) type SubscriptionHandler = Box<dyn FnMut(PublishEvent) -> bool + Send + Sync>;

pub struct TanukiConnection {
    endpoint: Endpoint<role::Client>,
    next_payload_id: AtomicU16,
    // key could be SubscriptionIdentifier if it implemented Ord
    sub_handlers: Mutex<BTreeMap<u32, SubscriptionHandler>>,
}

impl TanukiConnection {
    pub async fn connect(client_id: &str, addr: &str) -> Result<Arc<Self>> {
        // Create a client endpoint
        let endpoint = mqtt_ep::endpoint::Endpoint::<role::Client>::new(mqtt_ep::Version::V5_0);

        // Connect to TCP transport
        let tcp_stream = connect_helper::connect_tcp(addr, None).await?;
        let transport = TcpTransport::from_stream(tcp_stream);
        endpoint
            .attach(transport, mqtt_ep::endpoint::Mode::Client)
            .await?;

        // Send CONNECT packet
        let connect = v5_0::Connect::builder()
            .client_id(client_id)
            .unwrap()
            .build()
            .unwrap();

        endpoint.send(connect).await?;

        // Receive CONNACK
        let packet = endpoint.recv().await?;
        let connack: Connack = packet.try_into().map_err(Error::MqttPacketField)?;
        tracing::debug!("Received CONNACK: {connack:?}");

        Ok(TanukiConnection {
            endpoint,
            next_payload_id: AtomicU16::new(1),
            sub_handlers: Mutex::new(BTreeMap::new()),
        }
        .into())
    }

    fn next_payload_id(&self) -> u16 {
        loop {
            let id = self
                .next_payload_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            if id != 0 {
                break id;
            }
        }
    }

    fn next_subscription_id(&self) -> SubscriptionIdentifier {
        // max value is 2^28 - 1 (min value is also 1)
        SubscriptionIdentifier::new(self.next_payload_id() as u32).unwrap()
    }

    pub async fn recv_raw(&self) -> Result<mqtt_ep::packet::Packet> {
        let packet = self.endpoint.recv().await?;
        Ok(packet)
    }

    pub async fn recv(&self) -> Result<PublishEvent> {
        loop {
            let packet = self.recv_raw().await?;

            let publish: Result<Publish, _> = packet.try_into();
            if let Ok(publish) = publish {
                let sub_id = publish.props.iter().find_map(|p| {
                    if let Property::SubscriptionIdentifier(id) = p {
                        Some(id.clone())
                    } else {
                        None
                    }
                });

                let topic = Topic::from_str(publish.topic_name()).map_err(Error::BadTopic)?;

                let payload: serde_json::Value =
                    serde_json::from_slice(publish.payload().as_slice())?;

                break Ok(PublishEvent { sub_id, topic, payload });
            }
        }
    }

    pub async fn handle(&self) -> Result<Infallible> {
        loop {
            let event = self.recv().await?;

            tracing::debug!("Handling publish event: {event:#?}");

            if let Some(sub_id) = event.sub_id.clone() {
                let mut handlers = self.sub_handlers.lock().await;

                if let Some(handler) = handlers.get_mut(&sub_id.val()) {
                    let retain = handler(event);

                    if !retain {
                        tracing::warn!("Removing subscription handler for ID {}", sub_id.val());
                        handlers.remove(&sub_id.val());
                    }
                }
            }
        }
    }

    pub async fn raw_subscribe(&self, topic: &str) -> Result<SubscriptionIdentifier> {
        let sub_id = self.next_subscription_id();

        let subscribe = v5_0::Subscribe::builder()
            .packet_id(self.next_payload_id())
            .props(vec![Property::SubscriptionIdentifier(sub_id.clone())])
            .entries(vec![SubEntry::new(
                topic.to_string(),
                SubOpts::new().set_qos(Qos::AtLeastOnce),
            )?])
            .build()?;

        tracing::info!("Subscribing to topic '{topic}'");

        self.endpoint
            .register_packet_id(subscribe.packet_id())
            .await?;

        self.endpoint.send(subscribe).await?;

        Ok(sub_id)
    }

    pub async fn subscribe(&self, topic: Topic) -> Result<SubscriptionIdentifier> {
        self.raw_subscribe(&topic.to_string()).await
    }

    pub async fn subscribe_with_handler(
        &self,
        topic: Topic,
        handler: SubscriptionHandler,
    ) -> Result<()> {
        let sub_id = self.subscribe(topic).await?;

        let mut handlers = self.sub_handlers.lock().await;
        handlers.insert(sub_id.val(), handler);

        Ok(())
    }

    pub async fn publish(
        &self,
        topic: Topic,
        payload: impl Serialize,
        opts: PublishOpts,
    ) -> Result<()> {
        let payload = serde_json::to_string(&payload)?;

        tracing::debug!("Publishing to topic {topic}: {payload}");

        let publish = v5_0::Publish::builder()
            .topic_name(topic.to_string())?
            .payload(payload)
            .qos(opts.qos)
            .retain(opts.retain)
            .packet_id(self.next_payload_id())
            .build()?;

        tracing::debug!("Publishing MQTT message: {publish:#?}");

        self.endpoint
            .register_packet_id(publish.packet_id().unwrap())
            .await?;

        self.endpoint.send(publish).await?;

        Ok(())
    }

    pub async fn publish_entity_meta<T: MetaField>(&self, entity: EntityId, meta: T) -> Result<()> {
        self.publish(
            Topic::EntityMeta {
                entity,
                key: TanukiString::const_new(T::KEY),
            },
            meta,
            PublishOpts::metadata(),
        )
        .await
    }

    pub fn entity(self: &Arc<Self>, id: impl Into<EntityId>) -> Arc<TanukiEntity<User>> {
        Arc::new(TanukiEntity {
            id: id.into(),
            conn: self.clone(),
            _role: PhantomData,
        })
    }

    pub fn entity_cap<C: Capability<User>>(self: &Arc<Self>, id: impl Into<EntityId>) -> C {
        TanukiEntity::new(id.into(), self.clone()).capability::<C>()
    }

    pub async fn author_entity(
        self: &Arc<Self>,
        id: impl Into<EntityId>,
    ) -> Result<Arc<TanukiEntity<Authority>>> {
        let entity = TanukiEntity::new(id.into(), self.clone());

        entity.initialize().await?;

        Ok(entity)
    }
}

#[derive(Debug, Clone)]
pub struct PublishEvent {
    pub sub_id: Option<SubscriptionIdentifier>,
    pub topic: Topic,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy)]
pub struct PublishOpts {
    pub qos: Qos,
    pub retain: bool,
}

impl PublishOpts {
    pub const fn metadata() -> Self {
        Self { qos: Qos::AtLeastOnce, retain: true }
    }

    pub const fn entity_data() -> Self {
        Self { qos: Qos::AtLeastOnce, retain: true }
    }

    pub const fn event() -> Self {
        Self { qos: Qos::ExactlyOnce, retain: false }
    }

    pub const fn control() -> Self {
        Self { qos: Qos::ExactlyOnce, retain: false }
    }
}

pub struct TanukiEntity<R: EntityRole> {
    id: EntityId,
    conn: Arc<TanukiConnection>,
    _role: PhantomData<R>,
}

impl<R: EntityRole> TanukiEntity<R> {
    pub(crate) fn new(id: EntityId, conn: Arc<TanukiConnection>) -> Arc<Self> {
        Arc::new(Self { id, conn, _role: PhantomData })
    }

    pub fn id(&self) -> &EntityId {
        &self.id
    }

    pub fn connection(&self) -> Arc<TanukiConnection> {
        self.conn.clone()
    }
}

impl TanukiEntity<User> {
    pub fn capability<C: Capability<User>>(self: &Arc<Self>) -> C {
        C::from(TanukiCapability {
            entity: self.clone(),
            capability: C::ID.to_tanuki_string(),
        })
    }
}

impl TanukiEntity<Authority> {
    pub(crate) async fn initialize(&self) -> Result<()> {
        self.conn
            .publish_entity_meta(self.id.clone(), meta::EntityStatus::Online) // TODO: Init first
            .await?;

        Ok(())
    }

    // pub async fn status_online(&self) -> Result<()> {
    //     self.conn
    //         .publish_entity_meta(&self.id, meta::Status(EntityStatus::Online))
    //         .await
    // }

    pub async fn publish_meta(&self, meta: impl MetaField) -> Result<()> {
        self.conn.publish_entity_meta(self.id.clone(), meta).await
    }

    pub async fn author_capability<C: Capability<Authority>>(self: &Arc<Self>) -> Result<C> {
        let cap = C::from(TanukiCapability {
            entity: self.clone(),
            capability: C::ID.to_tanuki_string(),
        });

        cap.initialize(C::VERSION).await?;

        Ok(cap)
    }
}
