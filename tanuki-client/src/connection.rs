use crate::protocol::{ClientMessage, ServerMessage};

/// The largest WebSocket message either side accepts.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// How outgoing messages are encoded. Incoming messages are decoded by frame
/// type (text is JSON, binary is MessagePack), whatever the codec.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Codec {
    #[default]
    Json,
    MessagePack,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("WebSocket transport failed: {0}")]
    #[cfg(not(target_arch = "wasm32"))]
    Transport(#[from] tokio_tungstenite::tungstenite::Error),
    #[cfg(target_arch = "wasm32")]
    #[error("WebSocket transport failed: {0}")]
    Transport(#[from] gloo_net::websocket::WebSocketError),
    #[error("invalid JSON message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("MessagePack encode failed: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("MessagePack decode failed: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    /// An outgoing message would exceed [`MAX_MESSAGE_BYTES`]; nothing was sent
    /// and the connection is still usable. (An incoming oversized message is a
    /// transport error that ends the connection.)
    #[error("wire message exceeds {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    /// The server closed the connection with a code other than 1000 or 1001.
    /// The reason is a stable snake_case string such as `session_replaced`;
    /// see the close codes in `docs/protocol.md`.
    #[error("server closed WebSocket with code {code}: {reason}")]
    Closed { code: u16, reason: String },
}

/// One decoded incoming WebSocket event, the same on every target.
pub(crate) enum Incoming {
    Message(ServerMessage),
    /// Ping/pong and raw frames: transport plumbing, answered by the library.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "browsers answer control frames themselves")
    )]
    Control,
    /// A normal close (1000/1001) or end of stream.
    Closed,
}

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{Receiver, Sender};

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::{Receiver, Sender};

/// One WebSocket connection to `/v1/ws`, exchanging typed protocol messages.
///
/// This is the whole protocol layer: no hello automation, request IDs,
/// caching or reconnection. Send a `hello` first; the server answers it with a
/// correlated snapshot. Use [`Connection::split`] to receive in one task while
/// sending from others.
///
/// Keep receiving: on native targets the receiver also answers the server's
/// pings, and a connection that stops reading is closed after the server's
/// silence window (60 s by default).
pub struct Connection {
    sender: Sender,
    receiver: Receiver,
}

impl Connection {
    /// Opens a connection that sends JSON.
    pub async fn open(url: &str) -> Result<Self, ConnectionError> {
        Self::open_with_codec(url, Codec::Json).await
    }

    /// Opens a connection whose [`send`](Self::send) uses `codec`.
    pub async fn open_with_codec(url: &str, codec: Codec) -> Result<Self, ConnectionError> {
        let (sender, receiver) = Sender::open(url, codec).await?;
        Ok(Self { sender, receiver })
    }

    pub async fn send(&self, message: &ClientMessage) -> Result<(), ConnectionError> {
        self.sender.send(message).await
    }

    /// Sends one message with a specific codec. The server replies to a
    /// request in the codec of the frame that carried it.
    pub async fn send_with_codec(
        &self,
        message: &ClientMessage,
        codec: Codec,
    ) -> Result<(), ConnectionError> {
        self.sender.send_with_codec(message, codec).await
    }

    /// The next server message, or `None` after a normal close (1000/1001) or
    /// end of stream. Any other close is [`ConnectionError::Closed`]. Once this
    /// has returned `None` or an error, it keeps returning `None`.
    /// Cancellation safe.
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>, ConnectionError> {
        self.receiver.recv().await
    }

    /// Starts a normal close (code 1000). Keep calling [`recv`](Self::recv)
    /// until it returns `None` to observe the server's acknowledgement.
    pub async fn close(&self) -> Result<(), ConnectionError> {
        self.sender.close().await
    }

    /// Separates the halves. The [`Sender`] is cheap to clone and may be used
    /// from several tasks; the [`Receiver`] must keep being polled.
    pub fn split(self) -> (Sender, Receiver) {
        (self.sender, self.receiver)
    }
}
