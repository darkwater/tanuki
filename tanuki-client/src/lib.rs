#![doc = include_str!("../README.md")]
pub use jiff;
pub use tanuki_protocol as protocol;
pub use tanuki_protocol::{
    ChangeView, ClaimView, ClientName, ErrorCode, ErrorView, FromValue, NodeKind, NodeView,
    PayloadError, ProvenanceView, Selector, SessionMode, ToValue, TopicPath, UpdateView, Value,
    WireValue, from_value, to_value,
};

mod connection;
mod control;
mod driver;
mod listen;
mod observe;
mod runtime;
mod session;
mod topic;

pub use connection::{Codec, Connection, ConnectionError};
pub use control::{ControlError, Controller, Request};
pub use listen::{Event, Listener, Occurrence, OccurrenceError, Occurrences, Resync};
pub use observe::{Observer, Snapshot};
pub use session::{
    Batch, ClientError, Closed, Disconnect, Reconnect, Session, SessionBuilder, Status,
    StatusWatch, WriteReceipt,
};
pub use topic::{
    ClaimRelease, Command, CommandTopic, Current, Desired, DesiredTopic, DurationOutOfRange,
    Event as EventKind, EventTopic, Expiry, InputKind, InstantKind, IntoSelectors, Node, ReadError,
    SelectorSource, State, StateTopic, Topic, TopicKind,
};

#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
