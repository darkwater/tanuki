#![doc = include_str!("../README.md")]
pub use tanuki_protocol as protocol;
mod connection;
mod runtime;
pub use connection::{Codec, Connection, ConnectionError};
mod session;
pub use session::{ClientError, RawListener, Session, SessionEnd, SessionOptions, WriteReceipt};
pub use tanuki_protocol::{AppliedUpdate, ClientViewError, SelectedView};
mod payload;
pub use payload::{PayloadError, from_value, to_value};
mod topic;
pub use topic::{
    Batch, ClaimRelease, Command, CommandTopic, Current, DecodedNode, Desired, DesiredTopic, Event,
    EventTopic, ExpiryUpdate, InputTopicKind, NodeKind, ReadError, State, StateTopic, Topic,
    TopicKind,
};
mod observe;
pub use observe::{ObservationSnapshot, Observer};

#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
