use std::sync::Arc;

use tanuki::{
    TanukiConnection, TanukiEntity,
    capabilities::{Authority, buttons::Buttons, light::Light, on_off::OnOff},
    registry::Registry,
};
use tanuki_common::{
    capabilities::{light::LightCommand, on_off::OnOffCommand},
    meta,
};
use tokio_tungstenite::tungstenite::{self};

use self::{
    entity::{EntityDataMapping, EntityServiceMapping, MappedEntity, ServiceCall, ServiceMapping},
    hass::HomeAssistant,
    messages::{EventData, ServerError, StateEvent},
};
use crate::messages::{Packet, PacketId, ServerMessage};

pub mod entity;
mod hass;
mod messages;

type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("websocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("tanuki error: {0}")]
    Tanuki(#[from] tanuki::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("hass error: {0}")]
    Hass(ServerError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("authentication failed: {0}")]
    Authentication(String),
}

pub async fn bridge(
    tanuki: &str,
    host: &str,
    token: &str,
    mappings: Vec<MappedEntity>,
) -> Result<()> {
    let addr = format!("wss://{host}/api/websocket");
    let (hass, mut hass_rx) = HomeAssistant::connect(&addr, token).await?;
    let hass = Arc::new(hass);

    async fn entity_init(ent: &TanukiEntity<Authority>) -> tanuki::Result<()> {
        ent.publish_meta(meta::Provider("tanuki-hass".into())).await
    }

    let tanuki: Arc<TanukiConnection> = TanukiConnection::connect("tanuki-hass", tanuki).await?;

    let mut registry = Registry::new(tanuki.clone());

    let mappings = Arc::<[_]>::from(mappings.into_boxed_slice());

    tokio::spawn({
        let tanuki = tanuki.clone();

        async move {
            let Err(e) = tanuki.handle().await;
            tracing::error!("Error handling Tanuki messages: {e}");
        }
    });

    for MappedEntity { tanuki_id, from_hass: _, to_hass } in mappings.as_ref() {
        for EntityServiceMapping { hass_id, service } in to_hass {
            let hass = hass.clone();
            let hass_id = hass_id.clone();

            match *service {
                ServiceMapping::OnOff { domain } => {
                    let entity: &mut OnOff<Authority> =
                        registry.get(tanuki_id, entity_init).await?;

                    entity
                        .listen(move |cmd: OnOffCommand| {
                            let call = ServiceCall {
                                domain: domain.to_string(),
                                service: match cmd {
                                    OnOffCommand::On => "turn_on".to_string(),
                                    OnOffCommand::Off => "turn_off".to_string(),
                                    OnOffCommand::Toggle => "toggle".to_string(),
                                },
                                service_data: serde_json::Value::Null,
                            };

                            hass.call_service(call.target_entity(&hass_id));
                        })
                        .await
                        .unwrap(); // TODO: better handling?
                }
                ServiceMapping::Light => {
                    let entity: &mut Light<Authority> =
                        registry.get(tanuki_id, entity_init).await?;

                    entity
                        .listen(move |cmd: LightCommand| {
                            let call = ServiceCall {
                                domain: "light".to_string(),
                                service: match cmd.on {
                                    true => "turn_on".to_string(),
                                    false => "turn_off".to_string(),
                                },
                                service_data: if cmd.on
                                    && let Some(color) = cmd.color
                                {
                                    serde_json::json!({
                                        color.hass_service_data_key(): color.to_hass()
                                    })
                                } else {
                                    serde_json::Value::Null
                                },
                            };

                            hass.call_service(call.target_entity(&hass_id));
                        })
                        .await
                        .unwrap(); // TODO: better handling?
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    loop {
        let Some(packet) = hass_rx.recv().await else {
            panic!("Home Assistant connection closed");
        };

        match packet.payload {
            ServerMessage::Result { success, result, error } => {
                if !success {
                    return Err(Error::Hass(error.unwrap_or(ServerError {
                        code: "unknown".to_string(),
                        message: "success: false, but no error given".to_string(),
                    })));
                }

                // get_states result
                if let Ok(states) = serde_json::from_value::<Vec<StateEvent>>(result) {
                    for state in states {
                        tracing::debug!(
                            "Sensor '{}' is {} {}",
                            state.entity_id,
                            state.state.state,
                            state.state.attributes.unit_of_measurement,
                        );

                        for MappedEntity { tanuki_id, from_hass, to_hass: _ } in mappings.as_ref() {
                            for mapping in from_hass {
                                if let EntityDataMapping::State { from_id, map_to } = mapping {
                                    if from_id != &state.entity_id {
                                        continue;
                                    }

                                    map_to
                                        .propagate_state(
                                            &state.state,
                                            &mut registry,
                                            tanuki_id,
                                            entity_init,
                                        )
                                        .await?;

                                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                }
                            }
                        }
                    }
                }
            }
            ServerMessage::Event { event } => match event.data {
                EventData::StateChanged(sensor_event) => {
                    tracing::info!(
                        "Sensor '{}' changed from {} {} to {} {}",
                        sensor_event.entity_id,
                        sensor_event.old_state.state,
                        sensor_event.old_state.attributes.unit_of_measurement,
                        sensor_event.new_state.state,
                        sensor_event.new_state.attributes.unit_of_measurement,
                    );
                    for MappedEntity { tanuki_id, from_hass, to_hass: _ } in mappings.as_ref() {
                        for mapping in from_hass {
                            if let EntityDataMapping::State { from_id, map_to } = mapping {
                                if from_id != &sensor_event.entity_id {
                                    continue;
                                }

                                map_to
                                    .propagate_state(
                                        &sensor_event.new_state,
                                        &mut registry,
                                        tanuki_id,
                                        entity_init,
                                    )
                                    .await?;
                            }
                        }
                    }
                }
                EventData::ZhaEvent(zha_event) => {
                    tracing::info!("ZHA Event {zha_event:#?}");

                    for MappedEntity { tanuki_id, from_hass, to_hass: _ } in mappings.as_ref() {
                        for mapping in from_hass {
                            if let EntityDataMapping::ZhaCommands { device_ieee, translations } =
                                mapping
                            {
                                if device_ieee != &zha_event.device_ieee {
                                    continue;
                                }

                                for translation in translations {
                                    if translation.command != zha_event.command {
                                        continue;
                                    }

                                    let empty_map = serde_json::Map::new();

                                    let left_params =
                                        translation.params.as_object().unwrap_or(&empty_map);

                                    let right_params =
                                        zha_event.params.as_object().unwrap_or(&empty_map);

                                    dbg!((&left_params, &right_params));

                                    // are there any elements in left...
                                    let mismatch = left_params.iter().any(|(k, v)| {
                                        // ...where the matching element in right...
                                        match right_params.get(k) {
                                            // ...is different?
                                            Some(rv) => rv != v,
                                            // ...is missing?
                                            None => true,
                                        }
                                    });

                                    // then it's not a match
                                    if mismatch {
                                        continue;
                                    }

                                    match &translation.map_to {
                                        entity::CapEventMapping::Button { button, action } => {
                                            let sensor: &mut Buttons<Authority> =
                                                registry.get(tanuki_id, entity_init).await?;

                                            sensor.publish_action(button, *action).await?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}
