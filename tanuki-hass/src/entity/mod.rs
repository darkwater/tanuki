use serde::Serialize;
use tanuki::{Authority, TanukiEntity, registry::Registry};
use tanuki_common::{
    EntityId,
    capabilities::{
        light::{Brightness, Color, ColorMode, ColorProperty},
        on_off::On,
        sensor::{SensorPayload, SensorValue},
    },
};

use crate::messages::SensorState;

pub struct MappedEntity {
    pub tanuki_id: EntityId,
    pub from_hass: Vec<EntityDataMapping>,
    pub to_hass: Vec<EntityServiceMapping>,
}

pub struct EntityDataMapping {
    pub from_id: String,
    pub map_to: CapMapping,
}

pub enum CapMapping {
    Sensor { key: String, binary: bool },
    Light,
}

impl CapMapping {
    pub fn sensor(key: impl ToString) -> Self {
        CapMapping::Sensor { key: key.to_string(), binary: false }
    }

    pub fn binary_sensor(key: impl ToString) -> Self {
        CapMapping::Sensor { key: key.to_string(), binary: true }
    }

    pub(crate) async fn propagate_state(
        &self,
        state: &SensorState,
        registry: &mut Registry,
        tanuki_id: &EntityId,
        entity_init: impl AsyncFnOnce(&TanukiEntity<Authority>) -> tanuki::Result<()>,
    ) -> tanuki::Result<()> {
        match self {
            CapMapping::Sensor { key, binary } => {
                let sensor = registry.sensor(tanuki_id, entity_init).await?;

                let value = match binary {
                    false => {
                        let Ok(value) = state.state.parse() else {
                            tracing::warn!(
                                "Failed to parse sensor value '{}' as number",
                                state.state
                            );
                            return Ok(());
                        };
                        SensorValue::Number(value)
                    }
                    true => {
                        let value = match state.state.as_str() {
                            "on" => true,
                            "off" => false,
                            _ => {
                                tracing::warn!(
                                    "Failed to parse binary sensor value '{}' as boolean",
                                    state.state
                                );
                                return Ok(());
                            }
                        };
                        SensorValue::Boolean(value)
                    }
                };

                sensor
                    .publish(key.clone(), SensorPayload {
                        value,
                        unit: state.attributes.unit_of_measurement.clone().into(),
                        timestamp: state.last_updated,
                    })
                    .await
            }
            CapMapping::Light => {
                let on_off = registry.on_off(tanuki_id, entity_init).await?;

                let on = match state.state.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => {
                        tracing::warn!(
                            "Failed to parse light state value '{}' as boolean",
                            state.state
                        );
                        return Ok(());
                    }
                };

                on_off.publish(On(on)).await?;

                let light = registry.light(tanuki_id, async |_| unreachable!()).await?;

                if let Some(brightness) = state.attributes.brightness {
                    light.publish(Brightness(brightness as f32 / 255.0)).await?;
                }

                if let Some(color_mode) = state.attributes.color_mode
                    && color_mode != ColorMode::Brightness
                {
                    let color_list = match color_mode {
                        ColorMode::Rgbww => &state.attributes.rgbww_color[..],
                        ColorMode::Rgbw => &state.attributes.rgbw_color[..],
                        ColorMode::Rgb => &state.attributes.rgb_color[..],
                        ColorMode::Hs => &state.attributes.hs_color[..],
                        ColorMode::Xy => &state.attributes.xy_color[..],
                        ColorMode::ColorTemp => &[], // TODO
                        ColorMode::Brightness => &[],
                        ColorMode::OnOff => &[],
                    };

                    if let Some(color) = Color::from_slice(color_mode, color_list) {
                        light.publish(ColorProperty(color)).await?;
                    } else {
                        tracing::warn!(
                            "Failed to parse light color from mode {color_mode:?} and data {color_list:?}",
                        );
                    }
                }

                Ok(())
            }
        }
    }
}

pub struct EntityServiceMapping {
    pub hass_id: String,
    pub service: ServiceMapping,
}

pub enum ServiceMapping {
    OnOff { domain: &'static str },
    Light,
}

impl ServiceMapping {
    pub(crate) fn translate_command(
        &self,
        topic: &str,
        payload: &serde_json::Value,
    ) -> Option<ServiceCall> {
        match (self, topic, payload.as_str()) {
            (Self::OnOff { domain }, "command", Some(r#""on""#)) => Some(ServiceCall {
                domain: domain.to_string(),
                service: "turn_on".to_string(),
                service_data: serde_json::Value::Null,
            }),
            (Self::OnOff { domain }, "command", Some(r#""off""#)) => Some(ServiceCall {
                domain: domain.to_string(),
                service: "turn_off".to_string(),
                service_data: serde_json::Value::Null,
            }),
            (Self::Light, "color/set", Some(color)) => {
                use tanuki_common::capabilities::light::Color;

                let color: Color = serde_json::from_str(color).ok()?; // TODO: handle better

                Some(ServiceCall {
                    domain: "light".to_string(),
                    service: "turn_on".to_string(),
                    service_data: serde_json::json!({
                        color.hass_service_data_key(): color.to_hass()
                    }),
                })
            }
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ServiceCall {
    pub domain: String,
    pub service: String,
    pub service_data: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetedServiceCall {
    #[serde(flatten)]
    pub call: ServiceCall,
    pub target: ServiceCallTarget,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceCallTarget {
    EntityId(EntityId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_call_serde() {
        assert_eq!(
            serde_json::to_value(&TargetedServiceCall {
                call: ServiceCall {
                    domain: "light".to_string(),
                    service: "turn_on".to_string(),
                    service_data: serde_json::json!({ "brightness_pct": 24 }),
                },
                target: ServiceCallTarget::EntityId(EntityId::from("light.living_room")),
            })
            .unwrap(),
            serde_json::json!({
                "domain": "light",
                "service": "turn_on",
                "service_data": { "brightness_pct": 24 },
                "target": { "entity_id": "light.living_room" },
            }),
        );
    }
}
