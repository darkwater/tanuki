use compact_str::{CompactString, ToCompactString};
use tanuki_common::{Topic, capabilities::buttons::ButtonEvent};

use super::Capability;
use crate::{Authority, EntityRole, PublishOpts, Result, TanukiCapability, capability};

#[capability(id = tanuki_common::capabilities::ids::BUTTONS)]
pub struct Buttons<R: EntityRole> {
    cap: TanukiCapability<R>,
}

impl Buttons<Authority> {
    pub async fn publish_event(&self, key: impl ToCompactString, ev: ButtonEvent) -> Result<()> {
        self.cap.publish_raw(key, &ev, PublishOpts::event()).await
    }
}

impl<R: EntityRole> Buttons<R> {
    pub async fn listen(
        &self,
        listener: impl Fn(&str, ButtonEvent) + Send + Sync + 'static,
    ) -> Result<()> {
        self.cap
            .entity
            .conn
            .subscribe_with_handler(
                Topic::CapabilityData {
                    entity: self.entity.id().clone(),
                    capability: self.capability.clone(),
                    rest: CompactString::const_new("+"), // any button
                },
                Box::new(move |ev| {
                    match (ev.topic, serde_json::from_value::<ButtonEvent>(ev.payload)) {
                        (Topic::CapabilityData { rest, .. }, Ok(payload)) => {
                            listener(&rest, payload);
                        }
                        (Topic::CapabilityData { rest, .. }, Err(e)) => {
                            tracing::error!("Failed to deserialize event {rest}: {e}",);
                        }
                        (topic, _) => {
                            tracing::error!("Received property on unexpected topic: {topic}");
                        }
                    }
                    true
                }),
            )
            .await
    }
}
