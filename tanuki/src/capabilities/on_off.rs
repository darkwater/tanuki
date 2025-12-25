use tanuki_common::capabilities::on_off::OnOffProperty;

use super::Capability;
use crate::{Authority, EntityRole, PublishOpts, Result, TanukiCapability, capability};

#[capability(id = tanuki_common::capabilities::ids::ON_OFF)]
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

impl<R: EntityRole> OnOff<R> {
    pub async fn listen<T: OnOffProperty>(
        &self,
        listener: impl Fn(T) + Send + Sync + 'static,
    ) -> Result<()> {
        self.cap.listen(listener, false).await
    }

    pub async fn get<T: OnOffProperty + Send + 'static>(&self) -> Result<T> {
        self.cap.get().await
    }
}
