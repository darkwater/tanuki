#![doc = include_str!("../README.md")]
pub use tanuki_protocol as protocol;

mod client;
mod connection;
mod runtime;

pub use client::{
    Backoff, Client, ClientBuilder, ClientError, Closed, Disconnect, Event, Events, SnapshotCause,
    Status,
};
pub use connection::{Codec, Connection, ConnectionError, MAX_MESSAGE_BYTES, Receiver, Sender};

#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
