use core::sync::atomic::AtomicU32;
use std::collections::{HashMap, hash_map::Entry};

use futures::{SinkExt, Stream, StreamExt};
use serde::Deserialize;
use tanuki::{Authority, TanukiConnection, capabilities::sensor::Sensor};
use tanuki_common::{
    capabilities::sensor::{SensorPayload, SensorValue},
    meta,
};
use tokio_tungstenite::tungstenite::{self, Message};

use crate::messages::{
    AuthClientMessage, AuthServerMessage, ClientMessage, Packet, PacketId, ServerMessage,
};

pub mod entity;
pub mod messages;

type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("websocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("tanuki error: {0}")]
    Tanuki(#[from] tanuki::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("authentication failed: {0}")]
    Authentication(String),
}

pub async fn bridge(tanuki: &str, host: &str, token: &str) -> Result<()> {
    let addr = format!("wss://{host}/api/websocket");
    let (mut conn, res) = tokio_tungstenite::connect_async(addr).await?;

    tracing::debug!("WebSocket response: {res:?}");

    // Authentication phase
    async fn get_message(
        mut conn: impl Stream<Item = tungstenite::Result<Message>> + Unpin,
    ) -> Result<AuthServerMessage> {
        match conn.next().await {
            Some(Ok(Message::Text(txt))) => {
                serde_json::from_str::<AuthServerMessage>(&txt).map_err(Error::from)
            }
            Some(Ok(msg)) => Err(Error::Protocol(format!("expected text message, got: {:?}", msg))),
            Some(Err(e)) => Err(Error::WebSocket(e)),
            None => Err(Error::Protocol("connection closed unexpectedly".to_string())),
        }
    }

    let auth_required = get_message(&mut conn).await?;
    match auth_required {
        AuthServerMessage::AuthRequired { ha_version } => {
            tracing::info!("Connected to Home Assistant version {ha_version}");
        }
        _ => {
            return Err(Error::Protocol(format!(
                "expected AuthRequired message, got: {auth_required:?}",
            )));
        }
    }

    conn.send(Message::Text(
        serde_json::to_string(&AuthClientMessage::Auth { access_token: token.to_owned() })?.into(),
    ))
    .await?;

    let auth_required = get_message(&mut conn).await?;
    match auth_required {
        AuthServerMessage::AuthOk { ha_version: _ } => {
            tracing::info!("Authentication successful");
        }
        AuthServerMessage::AuthInvalid { message } => {
            return Err(Error::Authentication(message));
        }
        _ => {
            return Err(Error::Protocol(format!("expected auth outcome, got: {auth_required:?}")));
        }
    }

    let id = AtomicU32::new(1);

    conn.send(Message::Text(
        serde_json::to_string(&Packet {
            // TODO: 0 is an invalid packet id
            id: PacketId(id.fetch_add(1, std::sync::atomic::Ordering::SeqCst)),
            payload: ClientMessage::SubscribeEvents { event_type: None },
        })?
        .into(),
    ))
    .await?;

    let tanuki = TanukiConnection::connect("tanuki-hass", tanuki).await?;

    tokio::spawn({
        let tanuki = tanuki.clone();

        async move {
            loop {
                let packet = tanuki.recv_raw().await;
                tracing::debug!("Received packet: {packet:?}");
            }
        }
    });

    let mut devices = HashMap::<&'static str, Sensor<Authority>>::new();

    loop {
        let msg = match conn.next().await {
            Some(Ok(Message::Text(txt))) => serde_json::from_str::<ServerMessage>(&txt)?,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(msg)) => {
                tracing::warn!("expected text message, got: {:?}", msg);
                continue;
            }
            Some(Err(e)) => return Err(Error::WebSocket(e)),
            None => {
                return Err(Error::Protocol("connection closed unexpectedly".to_string()));
            }
        };

        tracing::info!("Received message: {msg:#?}");

        if let ServerMessage::Event { event } = msg {
            #[derive(Deserialize)]
            struct SensorStateEvent {
                entity_id: String,
                new_state: SensorState,
                old_state: SensorState,
            }

            #[derive(PartialEq, Deserialize)]
            struct SensorState {
                attributes: SensorAttributes,
                state: String,
            }

            #[derive(PartialEq, Deserialize)]
            struct SensorAttributes {
                #[serde(default)]
                unit_of_measurement: String,
            }

            struct EntityMapping {
                tanuki_entity: &'static str,
                cap: CapMapping,
            }

            enum CapMapping {
                Sensor { key: String, binary: bool },
            }

            impl CapMapping {
                fn sensor(key: impl ToString) -> Self {
                    CapMapping::Sensor { key: key.to_string(), binary: false }
                }

                fn binary_sensor(key: impl ToString) -> Self {
                    CapMapping::Sensor { key: key.to_string(), binary: true }
                }
            }

            let mappings = HashMap::<&str, EntityMapping>::from_iter([
                ("sensor.tv_voltage", EntityMapping {
                    tanuki_entity: "tapo_tv",
                    cap: CapMapping::sensor("voltage"),
                }),
                ("sensor.tv_current", EntityMapping {
                    tanuki_entity: "tapo_tv",
                    cap: CapMapping::sensor("current"),
                }),
                ("sensor.tv_current_consumption", EntityMapping {
                    tanuki_entity: "tapo_tv",
                    cap: CapMapping::sensor("current_consumption"),
                }),
                ("sensor.vindstyrka_temperature", EntityMapping {
                    tanuki_entity: "vindstyrka",
                    cap: CapMapping::sensor("temperature"),
                }),
                ("sensor.vindstyrka_humidity", EntityMapping {
                    tanuki_entity: "vindstyrka",
                    cap: CapMapping::sensor("humidity"),
                }),
                ("sensor.vindstyrka_pm2_5", EntityMapping {
                    tanuki_entity: "vindstyrka",
                    cap: CapMapping::sensor("pm2_5"),
                }),
                ("binary_sensor.motion_sensor_motion", EntityMapping {
                    tanuki_entity: "motion_sensor",
                    cap: CapMapping::binary_sensor("motion"),
                }),
            ]);

            if let Ok(sensor_event) = serde_json::from_value::<SensorStateEvent>(event.data) {
                tracing::info!(
                    "Sensor '{}' changed from {} {} to {} {}",
                    sensor_event.entity_id,
                    sensor_event.old_state.state,
                    sensor_event.old_state.attributes.unit_of_measurement,
                    sensor_event.new_state.state,
                    sensor_event.new_state.attributes.unit_of_measurement,
                );

                if let Some(EntityMapping { tanuki_entity, cap }) =
                    mappings.get(sensor_event.entity_id.as_str())
                {
                    let entry = devices.entry(tanuki_entity);
                    let sensor = match entry {
                        Entry::Occupied(entry) => entry,
                        Entry::Vacant(entry) => {
                            let entity = tanuki.owned_entity(tanuki_entity).await?;

                            // entity.publish_meta(meta::Name(name.into())).await?;
                            // entity
                            //     .publish_meta(meta::Type("BTHome Sensor".into()))
                            //     .await?;
                            entity
                                .publish_meta(meta::Provider("tanuki-hass".into()))
                                .await?;

                            let sensor = entity.capability::<Sensor<_>>().await?;
                            entry.insert_entry(sensor)
                        }
                    };

                    match &cap {
                        CapMapping::Sensor { key, binary } => {
                            let value = match binary {
                                false => {
                                    let Ok(value) = sensor_event.new_state.state.parse() else {
                                        tracing::warn!(
                                            "Failed to parse sensor value '{}' as number",
                                            sensor_event.new_state.state
                                        );
                                        continue;
                                    };
                                    SensorValue::Number(value)
                                }
                                true => {
                                    let value = match sensor_event.new_state.state.as_str() {
                                        "on" | "true" | "1" => true,
                                        "off" | "false" | "0" => false,
                                        _ => {
                                            tracing::warn!(
                                                "Failed to parse binary sensor value '{}' as boolean",
                                                sensor_event.new_state.state
                                            );
                                            continue;
                                        }
                                    };
                                    SensorValue::Boolean(value)
                                }
                            };

                            sensor
                                .get()
                                .publish(key.clone(), SensorPayload {
                                    value,
                                    unit: sensor_event
                                        .new_state
                                        .attributes
                                        .unit_of_measurement
                                        .into(),
                                    timestamp: event.time_fired.timestamp(),
                                })
                                .await?;
                        }
                    }
                }
            }
        }
    }
}
