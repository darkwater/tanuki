#![feature(macro_attr)]

use core::{marker::PhantomData, str::FromStr as _, sync::atomic::AtomicU16};
use std::sync::Arc;

use compact_str::{CompactString, ToCompactString};
use mqtt_endpoint_tokio::mqtt_ep::{
    self, Endpoint,
    packet::v5_0,
    role,
    transport::{TcpTransport, connect_helper},
};
use mqtt_protocol_core::mqtt::packet::{
    Qos, SubEntry, SubOpts,
    v5_0::{Connack, Publish},
};
use serde::Serialize;
use tanuki_common::{
    EntityId, EntityStatus, Topic,
    meta::{self, MetaField},
};

use crate::capabilities::{Capability, TanukiCapability};

pub mod capabilities;
pub mod log;
pub mod registry;

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

pub struct TanukiConnection {
    endpoint: Endpoint<role::Client>,
    next_payload_id: AtomicU16,
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

        let next_payload_id = AtomicU16::new(1);

        Ok(TanukiConnection { endpoint, next_payload_id }.into())
    }

    fn next_payload_id(&self) -> u16 {
        match self
            .next_payload_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        {
            0 => u16::MAX / 2, // zero is invalid, and hopefully this value is fine
            n => n,
        }
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
                let topic = Topic::from_str(publish.topic_name()).map_err(Error::BadTopic)?;

                let payload: serde_json::Value =
                    serde_json::from_slice(publish.payload().as_slice())?;

                break Ok(PublishEvent { topic, payload });
            }
        }
    }

    pub async fn raw_subscribe(&self, topic: &str) -> Result<()> {
        let subscribe = v5_0::Subscribe::builder()
            .packet_id(self.next_payload_id())
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

        Ok(())
    }

    pub async fn subscribe(&self, topic: Topic) -> Result<()> {
        self.raw_subscribe(&topic.to_string()).await
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
                key: CompactString::const_new(T::KEY),
            },
            meta,
            PublishOpts::metadata(),
        )
        .await
    }

    pub async fn owned_entity(
        self: &Arc<Self>,
        id: impl Into<EntityId>,
    ) -> Result<Arc<TanukiEntity<Authority>>> {
        let entity = TanukiEntity {
            id: id.into(),
            conn: self.clone(),
            _role: PhantomData,
        };
        entity.initialize().await?;
        Ok(Arc::new(entity))
    }
}

#[derive(Debug, Clone)]
pub struct PublishEvent {
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

pub trait EntityRole {
    const AUTHORITY: bool;
}
pub struct Authority;
pub struct User;
impl EntityRole for Authority {
    const AUTHORITY: bool = true;
}
impl EntityRole for User {
    const AUTHORITY: bool = false;
}

pub struct TanukiEntity<R: EntityRole> {
    id: EntityId,
    conn: Arc<TanukiConnection>,
    _role: PhantomData<R>,
}

impl TanukiEntity<Authority> {
    pub(crate) async fn initialize(&self) -> Result<()> {
        self.conn
            .publish_entity_meta(self.id.clone(), meta::Status(EntityStatus::Online)) // TODO: Init first
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
}

impl<R: EntityRole> TanukiEntity<R> {
    pub fn id(&self) -> &EntityId {
        &self.id
    }

    pub fn connection(&self) -> Arc<TanukiConnection> {
        self.conn.clone()
    }

    pub async fn capability<C: Capability<R>>(self: &Arc<Self>) -> Result<C> {
        let cap = C::from(TanukiCapability {
            entity: self.clone(),
            capability: C::ID.to_compact_string(),
        });

        if R::AUTHORITY {
            cap.initialize(C::VERSION).await?;
        }

        Ok(cap)
    }
}
