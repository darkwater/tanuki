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
    #[error("wire message exceeds {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("server closed WebSocket with code {code}: {reason}")]
    Closed { code: u16, reason: String },
}

/// One decoded incoming WebSocket event, the same on every target.
pub(crate) enum Incoming {
    Message(crate::protocol::ServerMessage),
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
pub use native::Connection;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{Message, decode_frame, encode_message};

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::Connection;
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{Message, decode_frame, encode_message};
