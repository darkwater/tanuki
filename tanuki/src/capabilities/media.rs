use tanuki_common::capabilities::media::{MediaCommand, MediaProperty};

use super::Capability;
use crate::{Authority, EntityRole, PublishOpts, Result, TanukiCapability, capability};

#[capability(id = tanuki_common::capabilities::ids::MEDIA)]
pub struct Media<R: EntityRole> {
    cap: TanukiCapability<R>,
}

impl Media<Authority> {
    pub async fn publish(&self, prop: impl MediaProperty) -> Result<()> {
        self.cap
            .publish_property(prop, PublishOpts::entity_data())
            .await
    }
}

impl<R: EntityRole> Media<R> {
    pub async fn command(&self, cmd: MediaCommand) -> Result<()> {
        self.cap.publish_property(cmd, PublishOpts::control()).await
    }

    pub async fn listen<T: MediaProperty>(
        &self,
        listener: impl Fn(T) + Send + Sync + 'static,
    ) -> Result<()> {
        self.cap.listen(listener, false).await
    }

    pub async fn get<T: MediaProperty + Send + 'static>(&self) -> Result<T> {
        self.cap.get().await
    }
}
