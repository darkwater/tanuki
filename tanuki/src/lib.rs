#![feature(macro_attr)]

use core::{marker::PhantomData, sync::atomic::AtomicU16};
use std::sync::Arc;

use compact_str::{CompactString, ToCompactString};
use mqtt_endpoint_tokio::mqtt_ep::{
    self, Endpoint,
    packet::v5_0,
    role,
    transport::{TcpTransport, connect_helper},
};
use mqtt_protocol_core::mqtt::packet::{Qos, v5_0::Connack};
use serde::Serialize;
use tanuki_common::{
    EntityId, EntityStatus, Property, Topic,
    meta::{self, MetaField},
};

use self::capabilities::CapabilityImpl;

pub mod capabilities;
pub mod log;
pub mod registry;

pub type Result<T> = std::result::Result<T, Error>;

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

#[derive(Clone, Copy, Debug)]
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

    pub async fn capability<C: CapabilityImpl<R>>(self: &Arc<Self>) -> Result<C> {
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

pub struct TanukiCapability<R: EntityRole> {
    entity: Arc<TanukiEntity<R>>,
    capability: CompactString,
}

impl<R: EntityRole> TanukiCapability<R> {
    pub fn entity(&self) -> Arc<TanukiEntity<R>> {
        self.entity.clone()
    }

    pub fn entity_id(&self) -> &EntityId {
        self.entity.id()
    }

    pub fn id(&self) -> &str {
        &self.capability
    }

    pub async fn initialize(&self, version: i32) -> Result<()> {
        self.publish_meta(meta::Version(version)).await?;

        Ok(())
    }

    pub async fn publish_raw(
        &self,
        topic: impl ToCompactString,
        payload: impl Serialize,
        opts: PublishOpts,
    ) -> Result<()> {
        let topic = Topic::CapabilityData {
            entity: self.entity.id().clone(),
            capability: self.capability.clone(),
            rest: topic.to_compact_string(),
        };

        self.entity.conn.publish(topic, payload, opts).await
    }

    pub async fn publish_property<T: Property>(
        &self,
        property: T,
        opts: PublishOpts,
    ) -> Result<()> {
        self.publish_raw(T::KEY, property, opts).await
    }

    pub async fn publish_meta<T: MetaField>(&self, meta: T) -> Result<()> {
        let topic = Topic::CapabilityMeta {
            entity: self.entity.id().clone(),
            capability: self.capability.clone(),
            key: CompactString::const_new(T::KEY),
        };

        self.entity
            .conn
            .publish(topic, meta, PublishOpts::metadata())
            .await
    }
}
