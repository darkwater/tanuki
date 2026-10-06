use crate::protocol::{ClientMessage, ServerMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message, protocol::WebSocketConfig},
};

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Codec {
    #[default]
    Json,
    MessagePack,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("WebSocket transport failed: {0}")]
    Transport(#[from] tungstenite::Error),
    #[error("invalid JSON message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("MessagePack encode failed: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("MessagePack decode failed: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("wire message exceeds {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("server closed WebSocket with code {code}: {reason}")]
    Closed { code: u16, reason: String },
}

/// Direct wire access. Hello, request IDs and all response handling belong to the caller.
pub struct Connection {
    pub(crate) socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    codec: Codec,
    ended: bool,
}
impl Connection {
    pub async fn open(url: &str) -> Result<Self, ConnectionError> {
        Self::open_with_codec(url, Codec::Json).await
    }
    pub async fn open_with_codec(url: &str, codec: Codec) -> Result<Self, ConnectionError> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(url, Some(config), false).await?;
        Ok(Self {
            socket,
            codec,
            ended: false,
        })
    }
    pub async fn send(&mut self, message: &ClientMessage) -> Result<(), ConnectionError> {
        self.send_with_codec(message, self.codec).await
    }
    pub async fn send_with_codec(
        &mut self,
        message: &ClientMessage,
        codec: Codec,
    ) -> Result<(), ConnectionError> {
        let frame = encode_message(message, codec)?;
        self.socket.send(frame).await?;
        Ok(())
    }
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>, ConnectionError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            let Some(frame) = self.socket.next().await else {
                self.ended = true;
                return Ok(None);
            };
            match frame? {
                Message::Text(text) => return Ok(Some(serde_json::from_str(&text)?)),
                Message::Binary(bytes) => return Ok(Some(rmp_serde::from_slice(&bytes)?)),
                Message::Ping(_) => self.socket.flush().await?,
                Message::Pong(_) | Message::Frame(_) => {}
                Message::Close(frame) => {
                    self.ended = true;
                    // Tungstenite queued the close acknowledgement.
                    let _ = self.socket.flush().await;
                    if let Some(frame) = frame {
                        let code = u16::from(frame.code);
                        if code != 1000 && code != 1001 {
                            return Err(ConnectionError::Closed {
                                code,
                                reason: frame.reason.to_string(),
                            });
                        }
                    }
                    return Ok(None);
                }
            }
        }
    }
    pub async fn close(&mut self) -> Result<(), ConnectionError> {
        self.socket.close(None).await?;
        self.ended = true;
        Ok(())
    }
}

pub(crate) fn encode_message(
    message: &ClientMessage,
    codec: Codec,
) -> Result<Message, ConnectionError> {
    let frame = match codec {
        Codec::Json => Message::Text(serde_json::to_string(message)?.into()),
        Codec::MessagePack => Message::Binary(rmp_serde::to_vec_named(message)?.into()),
    };
    if frame.len() > MAX_MESSAGE_BYTES {
        return Err(ConnectionError::MessageTooLarge);
    }
    Ok(frame)
}
