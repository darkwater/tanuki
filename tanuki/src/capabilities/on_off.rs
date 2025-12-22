use tanuki_common::capabilities::on_off::OnOffProperty;

use super::CapabilityImpl;
use crate::{Authority, EntityRole, PublishOpts, Result, TanukiCapability, capability};

#[capability(id = "tanuki.on_off")]
pub struct OnOff<R: EntityRole> {
    cap: TanukiCapability<R>,
}

impl OnOff<Authority> {
    pub async fn publish(&self, prop: impl OnOffProperty) -> Result<()> {
        self.cap
            .publish_property(prop, PublishOpts::entity_data())
            .await
    }
}
