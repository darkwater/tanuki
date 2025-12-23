use tanuki_common::capabilities::light::LightProperty;

use super::Capability;
use crate::{Authority, EntityRole, PublishOpts, Result, TanukiCapability, capability};

#[capability(id = tanuki_common::capabilities::ids::LIGHT)]
pub struct Light<R: EntityRole> {
    cap: TanukiCapability<R>,
}

impl Light<Authority> {
    pub async fn publish(&self, prop: impl LightProperty) -> Result<()> {
        self.cap
            .publish_property(prop, PublishOpts::entity_data())
            .await
    }
}
