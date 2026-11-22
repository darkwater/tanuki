use super::{Codec, ConnectionError, Incoming, MAX_MESSAGE_BYTES};
use crate::protocol::{ClientMessage, ServerMessage};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use std::sync::Arc;
use tokio::{net::TcpStream, sync::Mutex};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Sink = Arc<Mutex<SplitSink<Socket, Message>>>;

/// The sending half of a [`Connection`](super::Connection). Clones share the
/// socket; each message is sent whole.
#[derive(Clone)]
pub struct Sender {
    sink: Sink,
    codec: Codec,
}

/// The receiving half of a [`Connection`](super::Connection).
pub struct Receiver {
    stream: SplitStream<Socket>,
    // Tungstenite queues pongs and close acknowledgements on the socket; the
    // receiver flushes them through the shared sink.
    sink: Sink,
    ended: bool,
}

impl Sender {
    pub(super) async fn open(url: &str, codec: Codec) -> Result<(Self, Receiver), ConnectionError> {
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
        let (sink, stream) = socket.split();
        let sink = Arc::new(Mutex::new(sink));
        Ok((
            Self {
                sink: Arc::clone(&sink),
                codec,
            },
            Receiver {
                stream,
                sink,
                ended: false,
            },
        ))
    }

    /// Sends one message in this connection's codec.
    pub async fn send(&self, message: &ClientMessage) -> Result<(), ConnectionError> {
        self.send_with_codec(message, self.codec).await
    }

    pub async fn send_with_codec(
        &self,
        message: &ClientMessage,
        codec: Codec,
    ) -> Result<(), ConnectionError> {
        let frame = encode_message(message, codec)?;
        self.sink.lock().await.send(frame).await?;
        Ok(())
    }

    /// Starts a normal close (code 1000).
    pub async fn close(&self) -> Result<(), ConnectionError> {
        self.sink.lock().await.close().await?;
        Ok(())
    }
}

impl Receiver {
    /// See [`Connection::recv`](super::Connection::recv).
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>, ConnectionError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            match decode_frame(self.stream.next().await) {
                Ok(Incoming::Message(message)) => return Ok(Some(message)),
                // Tungstenite queued the pong; flushing sends it.
                Ok(Incoming::Control) => self.sink.lock().await.flush().await?,
                result => {
                    self.ended = true;
                    // Tungstenite queued any close acknowledgement.
                    let _ = self.sink.lock().await.flush().await;
                    return result.map(|_| None);
                }
            }
        }
    }
}

fn encode_message(message: &ClientMessage, codec: Codec) -> Result<Message, ConnectionError> {
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
fn decode_frame(
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
