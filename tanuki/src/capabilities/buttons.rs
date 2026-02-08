use serde::{Deserialize, Serialize};
use tanuki_common::{
    EntityId, TanukiString, ToTanukiString, Topic, capabilities::buttons::ButtonAction,
};

use super::{Capability, User};
use crate::{
    Authority, EntityRole, PublishEvent, PublishOpts, Result, TanukiCapability, capability,
};

#[capability(id = tanuki_common::capabilities::ids::BUTTONS)]
pub struct Buttons<R: EntityRole = User> {
    cap: TanukiCapability<R>,
}

impl Buttons<Authority> {
    pub async fn publish_action(&self, key: impl ToTanukiString, ev: ButtonAction) -> Result<()> {
        self.cap.publish_raw(key, &ev, PublishOpts::event()).await
    }
}

pub struct ButtonEvent {
    pub entity: EntityId,
    pub name: ButtonName,
    pub action: ButtonAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonName {
    On,
    Off,
    #[serde(untagged)]
    Other(String),
}

impl From<String> for ButtonName {
    fn from(value: String) -> Self {
        Self::deserialize(serde_json::Value::String(value)).unwrap()
    }
}

impl TryFrom<&PublishEvent> for ButtonEvent {
    type Error = ();

    fn try_from(publish: &PublishEvent) -> Result<Self, Self::Error> {
        if let PublishEvent {
            topic: Topic::CapabilityData { entity, capability, rest },
            payload,
            ..
        } = publish
            && capability == <Buttons>::ID
        {
            let name = ButtonName::from(rest.to_string());

            if let Ok(action) = ButtonAction::deserialize(payload) {
                Ok(ButtonEvent { entity: entity.clone(), name, action })
            } else {
                tracing::error!(
                    %entity,
                    %capability,
                    button = ?name,
                    action = %payload,
                    "Failed to deserialize button action",
                );

                Err(())
            }
        } else {
            Err(())
        }
    }
}

impl<R: EntityRole> Buttons<R> {
    pub async fn listen(
        &self,
        listener: impl Fn(&str, ButtonAction) + Send + Sync + 'static,
    ) -> Result<()> {
        self.cap
            .entity
            .conn
            .subscribe_with_handler(
                Topic::CapabilityData {
                    entity: self.entity.id().clone(),
                    capability: self.capability.clone(),
                    rest: TanukiString::const_new("+"), // any button
                },
                Box::new(move |ev| {
                    match (ev.topic, serde_json::from_value::<ButtonAction>(ev.payload)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_button_names() {
        assert_eq!(ButtonName::from("on".to_string()), ButtonName::On);
        assert_eq!(ButtonName::from("off".to_string()), ButtonName::Off);
        assert_eq!(ButtonName::from("".to_string()), ButtonName::Other("".to_string()));
        assert_eq!(ButtonName::from("asdf".to_string()), ButtonName::Other("asdf".to_string()));
    }
}
