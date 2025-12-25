use serde::Serialize;
use tanuki::{
    TanukiEntity,
    capabilities::{Authority, light::Light, on_off::OnOff, sensor::Sensor},
    registry::Registry,
};
use tanuki_common::{
    EntityId,
    capabilities::{
        buttons::ButtonEvent,
        light::{Color, ColorMode, LightState},
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

pub enum EntityDataMapping {
    State {
        from_id: String,
        map_to: CapMapping,
    },
    ZhaCommands {
        device_ieee: String,
        translations: Vec<ZhaEventTranslation>,
    },
}

pub enum CapMapping {
    Sensor { key: String, binary: bool },
    Light,
}

pub struct ZhaEventTranslation {
    pub command: String,
    pub params: serde_json::Value,
    pub map_to: CapEventMapping,
}

pub enum CapEventMapping {
    Button { button: String, event: ButtonEvent },
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
                let sensor: &mut Sensor<Authority> = registry.get(tanuki_id, entity_init).await?;

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

                let brightness = state
                    .attributes
                    .brightness
                    .map(|b| (b as f32 / 254.0).clamp(0., 1.));

                fn get_color<const N: usize>(opt: &Option<[f32; N]>) -> &[f32] {
                    match opt {
                        Some(slice) => slice,
                        None => &[],
                    }
                }

                let color = state.attributes.color_mode.and_then(|color_mode| {
                    let color_list = match color_mode {
                        ColorMode::Rgbww => get_color(&state.attributes.rgbww_color),
                        ColorMode::Rgbw => get_color(&state.attributes.rgbw_color),
                        ColorMode::Rgb => get_color(&state.attributes.rgb_color),
                        ColorMode::Hs => get_color(&state.attributes.hs_color),
                        ColorMode::Xy => get_color(&state.attributes.xy_color),
                        _ => &[], // TODO: handle more cases
                    };

                    Color::from_slice(color_mode, color_list)
                });

                registry
                    .get::<Light<Authority>>(tanuki_id, entity_init)
                    .await?
                    .publish(LightState { on, brightness, color })
                    .await?;

                registry
                    .get::<OnOff<Authority>>(tanuki_id, async |_| unreachable!())
                    .await?
                    .publish(On(on))
                    .await?;

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

#[derive(Debug, Serialize)]
pub(crate) struct ServiceCall {
    pub domain: String,
    pub service: String,
    pub service_data: serde_json::Value,
}

impl ServiceCall {
    pub fn target_entity(self, entity_id: impl ToString) -> TargetedServiceCall {
        TargetedServiceCall {
            call: self,
            target: ServiceCallTarget::EntityId(entity_id.to_string()),
        }
    }
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
    EntityId(String),
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
                target: ServiceCallTarget::EntityId("light.living_room".to_owned()),
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
