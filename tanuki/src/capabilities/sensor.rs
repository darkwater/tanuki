use compact_str::ToCompactString;
use tanuki_common::capabilities::sensor::SensorPayload;

use super::CapabilityImpl;
use crate::{Authority, EntityRole, PublishOpts, Result, TanukiCapability, capability};

#[capability(id = "tanuki.sensor")]
pub struct Sensor<R: EntityRole> {
    cap: TanukiCapability<R>,
}

impl Sensor<Authority> {
    pub async fn publish(&self, key: impl ToCompactString, payload: SensorPayload) -> Result<()> {
        self.cap
            .publish_raw(key, &payload, PublishOpts::entity_data())
            .await
    }
}
