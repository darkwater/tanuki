use super::{Codec, ConnectionError, MAX_MESSAGE_BYTES};
use crate::protocol::{ClientMessage, ServerMessage};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
pub(crate) use gloo_net::websocket::Message;
use gloo_net::websocket::{State, WebSocketError, futures::WebSocket};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Direct browser WebSocket access, using the same wire messages as native clients.
pub struct Connection {
    pub(crate) socket: Socket,
    codec: Codec,
    ended: bool,
}
impl Connection {
    pub async fn open(url: &str) -> Result<Self, ConnectionError> {
        Self::open_with_codec(url, Codec::Json).await
    }
    pub async fn open_with_codec(url: &str, codec: Codec) -> Result<Self, ConnectionError> {
        let ws = WebSocket::open(url).map_err(WebSocketError::MessageSendError)?;
        let mut socket = Socket(Some(ws));
        // Gloo's constructor starts the handshake; readiness waits for its open event.
        std::future::poll_fn(|cx| Pin::new(&mut socket).poll_ready(cx)).await?;
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
        self.socket.send(encode_message(message, codec)?).await?;
        Ok(())
    }
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>, ConnectionError> {
        if self.ended {
            return Ok(None);
        }
        let result = decode_frame(self.socket.next().await);
        if !matches!(result, Ok(Some(_))) {
            self.ended = true;
        }
        result
    }
    pub async fn close(&mut self) -> Result<(), ConnectionError> {
        self.socket.close().await?;
        self.ended = true;
        Ok(())
    }
}

// Gloo's Sink::poll_close is a no-op. Own the socket in an Option so closing a
// direct connection or the session writer actually requests a browser close.
pub(crate) struct Socket(Option<WebSocket>);
impl Stream for Socket {
    type Item = Result<Message, WebSocketError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.0.as_mut() {
            Some(ws) => Pin::new(ws).poll_next(cx),
            None => Poll::Ready(None),
        }
    }
}
impl Sink<Message> for Socket {
    type Error = WebSocketError;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let Some(ws) = self.0.as_mut() else {
            return Poll::Ready(Err(WebSocketError::ConnectionError));
        };
        match ws.state() {
            State::Closed | State::Closing => Poll::Ready(Err(WebSocketError::ConnectionError)),
            _ => Pin::new(ws).poll_ready(cx),
        }
    }
    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        Pin::new(self.0.as_mut().ok_or(WebSocketError::ConnectionError)?).start_send(message)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.0.as_mut() {
            Some(ws) => Pin::new(ws).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }
    fn poll_close(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(match self.0.take() {
            Some(ws) => ws
                .close(Some(1000), None)
                .map_err(WebSocketError::MessageSendError),
            None => Ok(()),
        })
    }
}

pub(crate) fn encode_message(
    message: &ClientMessage,
    codec: Codec,
) -> Result<Message, ConnectionError> {
    let frame = match codec {
        Codec::Json => Message::Text(serde_json::to_string(message)?),
        Codec::MessagePack => Message::Bytes(rmp_serde::to_vec_named(message)?),
    };
    let len = match &frame {
        Message::Text(text) => text.len(),
        Message::Bytes(bytes) => bytes.len(),
    };
    if len > MAX_MESSAGE_BYTES {
        return Err(ConnectionError::MessageTooLarge);
    }
    Ok(frame)
}

pub(crate) fn decode_frame(
    frame: Option<Result<Message, WebSocketError>>,
) -> Result<Option<ServerMessage>, ConnectionError> {
    match frame {
        Some(Ok(Message::Text(text))) => {
            if text.len() > MAX_MESSAGE_BYTES {
                return Err(ConnectionError::MessageTooLarge);
            }
            Ok(Some(serde_json::from_str(&text)?))
        }
        Some(Ok(Message::Bytes(bytes))) => {
            if bytes.len() > MAX_MESSAGE_BYTES {
                return Err(ConnectionError::MessageTooLarge);
            }
            Ok(Some(crate::protocol::decode_messagepack(&bytes)?))
        }
        Some(Err(WebSocketError::ConnectionClose(close))) => {
            if close.code == 1000 || close.code == 1001 {
                Ok(None)
            } else {
                Err(ConnectionError::Closed {
                    code: close.code,
                    reason: close.reason,
                })
            }
        }
        Some(Err(error)) => Err(error.into()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gloo_net::websocket::events::CloseEvent;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn browser_decode_rejects_oversized_frames_and_trailing_messagepack() {
        for frame in [
            Message::Text("x".repeat(MAX_MESSAGE_BYTES + 1)),
            Message::Bytes(vec![0; MAX_MESSAGE_BYTES + 1]),
        ] {
            assert!(matches!(
                decode_frame(Some(Ok(frame))),
                Err(ConnectionError::MessageTooLarge)
            ));
        }
        let message = ServerMessage::Snapshot {
            request_id: crate::protocol::RequestId::new("hello".into()),
            sequence: 0,
            nodes: Default::default(),
            warnings: vec![],
        };
        let mut bytes = rmp_serde::to_vec_named(&message).unwrap();
        bytes.push(0xc0);
        assert!(matches!(
            decode_frame(Some(Ok(Message::Bytes(bytes)))),
            Err(ConnectionError::Decode(_))
        ));
        assert!(matches!(
            decode_frame(Some(Ok(Message::Text("{".into())))),
            Err(ConnectionError::Json(_))
        ));
    }

    #[wasm_bindgen_test]
    fn browser_close_codes_distinguish_clean_and_abnormal_termination() {
        for code in [1000, 1001, 1006, 1013, 4001] {
            let frame = Some(Err(WebSocketError::ConnectionClose(CloseEvent {
                code,
                reason: "fixture".into(),
                was_clean: code != 1006,
            })));
            if code == 1000 || code == 1001 {
                assert!(decode_frame(frame).unwrap().is_none());
            } else {
                assert!(
                    matches!(decode_frame(frame), Err(ConnectionError::Closed { code: actual, reason }) if actual == code && reason == "fixture")
                );
            }
        }
    }
}
