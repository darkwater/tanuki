use core::sync::atomic::AtomicU32;
use std::sync::Arc;

use futures::{SinkExt, Stream, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::{self, Message};

use crate::{
    Error, Packet, PacketId, Result,
    entity::TargetedServiceCall,
    messages::{AuthClientMessage, AuthServerMessage, ClientMessage, ServerMessage},
};

pub struct HomeAssistant {
    tx: UnboundedSender<ClientMessage>,
}

impl HomeAssistant {
    pub async fn connect(
        addr: &str,
        token: &str,
    ) -> Result<(Self, UnboundedReceiver<Packet<ServerMessage>>)> {
        let (mut conn, res) = tokio_tungstenite::connect_async(addr).await?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        tracing::debug!("WebSocket response: {res:?}");

        // Authentication phase
        async fn get_message(
            mut conn: impl Stream<Item = tungstenite::Result<Message>> + Unpin,
        ) -> Result<AuthServerMessage> {
            match conn.next().await {
                Some(Ok(Message::Text(txt))) => {
                    serde_json::from_str::<AuthServerMessage>(&txt).map_err(Error::from)
                }
                Some(Ok(msg)) => {
                    Err(Error::Protocol(format!("expected text message, got: {:?}", msg)))
                }
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
            serde_json::to_string(&AuthClientMessage::Auth { access_token: token.to_owned() })?
                .into(),
        ))
        .await?;

        let auth_result = get_message(&mut conn).await?;
        match auth_result {
            AuthServerMessage::AuthOk { ha_version: _ } => {
                tracing::info!("Authentication successful");
            }
            AuthServerMessage::AuthInvalid { message } => {
                return Err(Error::Authentication(message));
            }
            _ => {
                return Err(Error::Protocol(format!(
                    "expected auth outcome, got: {auth_result:?}"
                )));
            }
        }

        let (mut conn_tx, mut conn_rx) = conn.split();

        tokio::spawn(async move {
            let id = AtomicU32::new(1);
            let next_id = move || {
                PacketId(match id.fetch_add(1, std::sync::atomic::Ordering::Relaxed) {
                    0 => id.fetch_add(1, std::sync::atomic::Ordering::Relaxed), // skip 0
                    n => n,
                })
            };
            let next_id = Arc::new(next_id);

            while let Some(msg) = rx.recv().await {
                let msg = Message::Text(
                    serde_json::to_string(&Packet { id: next_id(), payload: msg })
                        .unwrap()
                        .into(),
                );
                if let Err(e) = conn_tx.send(msg).await {
                    tracing::error!("Error sending message to Home Assistant: {e}");
                    break;
                }
            }
        });

        tx.send(ClientMessage::SubscribeEvents { event_type: None })
            .unwrap();

        tx.send(ClientMessage::GetStates).unwrap();

        let (packet_tx, packet_rx) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            loop {
                let packet = match conn_rx.next().await {
                    Some(Ok(Message::Text(txt))) => {
                        serde_json::from_str::<Packet<ServerMessage>>(&txt)
                            .expect("failed to parse server message")
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(msg)) => {
                        tracing::warn!("expected text message, got: {:?}", msg);
                        continue;
                    }
                    Some(Err(e)) => {
                        panic!("WebSocket error: {}", e);
                    }
                    None => {
                        panic!("connection closed unexpectedly");
                    }
                };

                tracing::info!("Received message: {packet:#?}");

                packet_tx.send(packet).expect("packet receiver dropped");
            }
        });

        Ok((Self { tx }, packet_rx))
    }

    pub fn call_service(&self, call: TargetedServiceCall) {
        self.tx
            .send(ClientMessage::CallService {
                domain: call.call.domain,
                service: call.call.service,
                service_data: call.call.service_data,
                target: call.target,
            })
            .unwrap();
    }
}
