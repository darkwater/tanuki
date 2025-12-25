use tanuki_common::capabilities::light::{LightCommand, LightProperty};

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

impl<R: EntityRole> Light<R> {
    pub async fn command(&self, cmd: LightCommand) -> Result<()> {
        self.cap.publish_property(cmd, PublishOpts::control()).await
    }

    pub async fn listen<T: LightProperty>(
        &self,
        listener: impl Fn(T) + Send + Sync + 'static,
    ) -> Result<()> {
        self.cap.listen(listener, false).await
    }

    pub async fn get<T: LightProperty + Send + 'static>(&self) -> Result<T> {
        self.cap.get().await
    }
}
