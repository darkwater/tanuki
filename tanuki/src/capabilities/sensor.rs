use serde::Deserialize as _;
use tanuki_common::{
    EntityId, TanukiString, ToTanukiString, Topic, capabilities::sensor::SensorPayload,
};

use super::{Capability, User};
use crate::{
    Authority, EntityRole, PublishEvent, PublishOpts, Result, TanukiCapability, capability,
};

#[capability(id = tanuki_common::capabilities::ids::SENSOR)]
pub struct Sensor<R: EntityRole = User> {
    cap: TanukiCapability<R>,
}

impl Sensor<Authority> {
    pub async fn publish(&self, key: impl ToTanukiString, payload: SensorPayload) -> Result<()> {
        self.cap
            .publish_raw(key, &payload, PublishOpts::entity_data())
            .await
    }
}

pub struct SensorEvent {
    pub entity: EntityId,
    pub key: TanukiString,
    pub payload: SensorPayload,
}

impl SensorEvent {
    pub fn as_str_tuple(&self) -> (&str, &str) {
        (self.entity.as_str(), self.key.as_str())
    }
}

impl TryFrom<&PublishEvent> for SensorEvent {
    type Error = ();

    fn try_from(publish: &PublishEvent) -> Result<Self, Self::Error> {
        if let PublishEvent {
            topic: Topic::CapabilityData { entity, capability, rest: key },
            payload,
            ..
        } = publish
            && capability == <Sensor>::ID
        {
            if let Ok(payload) = SensorPayload::deserialize(payload) {
                Ok(SensorEvent {
                    entity: entity.clone(),
                    key: key.clone(),
                    payload,
                })
            } else {
                tracing::error!(
                    %entity,
                    %capability,
                    sensor = %key,
                    %payload,
                    "Failed to deserialize sensor payload"
                );

                Err(())
            }
        } else {
            Err(())
        }
    }
}
