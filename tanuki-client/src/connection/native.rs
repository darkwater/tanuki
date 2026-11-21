use super::{Codec, ConnectionError, Incoming, MAX_MESSAGE_BYTES};
use crate::protocol::{ClientMessage, ServerMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::protocol::WebSocketConfig};

pub(crate) use tokio_tungstenite::tungstenite::Message;

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
        // rustls picks a provider from crate features only when exactly one is
        // enabled; another crate enabling aws-lc-rs would make wss:// panic.
        // Install ring unless the application already installed a provider.
        #[cfg(feature = "rustls")]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
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
            match decode_frame(self.socket.next().await) {
                Ok(Incoming::Message(message)) => return Ok(Some(message)),
                // Tungstenite queued the pong; flushing sends it.
                Ok(Incoming::Control) => self.socket.flush().await?,
                result => {
                    self.ended = true;
                    // Tungstenite queued any close acknowledgement.
                    let _ = self.socket.flush().await;
                    return result.map(|_| None);
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

/// Decodes one incoming frame. Message size is limited by the socket config.
pub(crate) fn decode_frame(
    frame: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> Result<Incoming, ConnectionError> {
    match frame {
        None => Ok(Incoming::Closed),
        Some(Err(source)) => Err(ConnectionError::Transport(source)),
        Some(Ok(Message::Text(text))) => Ok(Incoming::Message(serde_json::from_str(&text)?)),
        Some(Ok(Message::Binary(bytes))) => Ok(Incoming::Message(
            crate::protocol::decode_messagepack(&bytes)?,
        )),
        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => Ok(Incoming::Control),
        Some(Ok(Message::Close(frame))) => match frame {
            Some(frame) if !matches!(u16::from(frame.code), 1000 | 1001) => {
                Err(ConnectionError::Closed {
                    code: frame.code.into(),
                    reason: frame.reason.to_string(),
                })
            }
            _ => Ok(Incoming::Closed),
        },
    }
}
