use core::sync::atomic::AtomicU32;
use std::sync::Arc;

use futures::{SinkExt, Stream, StreamExt};
use tanuki::{TanukiConnection, TanukiEntity, capabilities::Authority, registry::Registry};
use tanuki_common::{Topic, meta};
use tokio_tungstenite::tungstenite::{self, Message};

use self::{
    entity::{EntityDataMapping, EntityServiceMapping, MappedEntity, ServiceCallTarget},
    messages::{StateChangeEvent, StateEvent},
};
use crate::messages::{
    AuthClientMessage, AuthServerMessage, ClientMessage, Packet, PacketId, ServerMessage,
};

pub mod entity;
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
    let next_id = move || {
        PacketId(match id.fetch_add(1, std::sync::atomic::Ordering::Relaxed) {
            0 => id.fetch_add(1, std::sync::atomic::Ordering::Relaxed), // skip 0
            n => n,
        })
    };

    conn.send(Message::text(serde_json::to_string(&Packet {
        id: next_id(),
        payload: ClientMessage::SubscribeEvents { event_type: None },
    })?))
    .await?;

    let get_states_id = next_id();
    conn.send(Message::text(serde_json::to_string(&Packet {
        id: get_states_id,
        payload: ClientMessage::GetStates,
    })?))
    .await?;

    let tanuki: Arc<TanukiConnection> = TanukiConnection::connect("tanuki-hass", tanuki).await?;

    let mappings = Arc::<[_]>::from(mappings.into_boxed_slice());

    let (mut conn_tx, mut conn_rx) = conn.split();

    tokio::spawn({
        let tanuki = tanuki.clone();
        let mappings = mappings.clone();

        tanuki.subscribe(Topic::CAPABILITY_DATA_WILDCARD).await?;

        async move {
            loop {
                let packet = tanuki.recv().await;
                tracing::info!("Received message: {packet:?}");

                let Ok(packet) = packet else {
                    continue;
                };

                if let Topic::CapabilityData { entity, capability, rest } = packet.topic {
                    for MappedEntity { tanuki_id, from_hass: _, to_hass } in mappings.as_ref() {
                        if tanuki_id != &entity {
                            continue;
                        }

                        for EntityServiceMapping { hass_id, service } in to_hass {
                            let cmd =
                                service.translate_command(&capability, &rest, &packet.payload);

                            if let Some(cmd) = cmd {
                                tracing::info!("{hass_id} <- {cmd:#?}");

                                // TODO
                                conn_tx
                                    .send(Message::text(
                                        serde_json::to_string(&Packet {
                                            id: next_id(),
                                            payload: ClientMessage::CallService {
                                                domain: cmd.domain,
                                                service: cmd.service,
                                                service_data: cmd.service_data,
                                                target: ServiceCallTarget::EntityId(
                                                    hass_id.clone(),
                                                ),
                                            },
                                        })
                                        .unwrap(),
                                    ))
                                    .await
                                    .unwrap();
                            }
                        }
                    }
                }
            }
        }
    });

    async fn entity_init(ent: &TanukiEntity<Authority>) -> tanuki::Result<()> {
        ent.publish_meta(meta::Provider("tanuki-hass".into())).await
    }

    let mut registry = Registry::new(tanuki);

    loop {
        let packet = match conn_rx.next().await {
            Some(Ok(Message::Text(txt))) => serde_json::from_str::<Packet<ServerMessage>>(&txt)?,
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

        tracing::info!("Received message: {packet:#?}");

        match packet.payload {
            ServerMessage::Result { success, result, error } => {
                if !success {
                    return Err(Error::Protocol(format!("Request failed: {:?}", error)));
                }

                if packet.id == get_states_id {
                    let states: Vec<StateEvent> = serde_json::from_value(result)?;
                    for state in states {
                        tracing::debug!(
                            "Sensor '{}' is {} {}",
                            state.entity_id,
                            state.state.state,
                            state.state.attributes.unit_of_measurement,
                        );

                        for MappedEntity { tanuki_id, from_hass, to_hass: _ } in mappings.as_ref() {
                            for EntityDataMapping { from_id, map_to } in from_hass {
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
            ServerMessage::Event { event } => {
                if let Ok(sensor_event) = serde_json::from_value::<StateChangeEvent>(event.data) {
                    tracing::info!(
                        "Sensor '{}' changed from {} {} to {} {}",
                        sensor_event.entity_id,
                        sensor_event.old_state.state,
                        sensor_event.old_state.attributes.unit_of_measurement,
                        sensor_event.new_state.state,
                        sensor_event.new_state.attributes.unit_of_measurement,
                    );

                    for MappedEntity { tanuki_id, from_hass, to_hass: _ } in mappings.as_ref() {
                        for EntityDataMapping { from_id, map_to } in from_hass {
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
        }
    }
}
