use crate::{ClientName, RequestId, Selector, TopicPath, Value};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        request_id: RequestId,
        client: ClientName,
        selectors: Vec<Selector>,
        /// Omitted on the wire means [`SessionMode::Managed`].
        session: SessionMode,
    },
    Write {
        request_id: RequestId,
        operations: Vec<WireOperation>,
    },
    /// Replace the connection's selection. Answered by a correlated snapshot of
    /// the new selection; updates after it follow the new selection only.
    Select {
        request_id: RequestId,
        selectors: Vec<Selector>,
    },
}

/// What a WebSocket hello establishes besides its subscription.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// A managed session: it can claim inputs, and a later managed hello with
    /// the same client name replaces it.
    #[default]
    Managed,
    /// Attributed like a stateless HTTP request: it cannot claim inputs and
    /// never replaces or is replaced by another connection.
    Stateless,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WireOperation {
    PublishState {
        topic: TopicPath,
        value: Value,
        #[serde(default)]
        expiry: Option<WireExpiry>,
    },
    PublishEvent {
        topic: TopicPath,
        value: Value,
    },
    DefineInput {
        topic: TopicPath,
        kind: WireInputKind,
    },
    ClaimInput {
        topic: TopicPath,
        release: WireRelease,
    },
    SubmitDesired {
        topic: TopicPath,
        value: Value,
        #[serde(default)]
        expiry: Option<WireExpiry>,
    },
    SubmitCommand {
        topic: TopicPath,
        value: Value,
    },
    ClearDesired {
        topic: TopicPath,
    },
    RemoveNode {
        topic: TopicPath,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireInputKind {
    Desired,
    Command,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WireExpiry {
    Preserve,
    Clear,
    /// A fixed ISO 8601 duration such as `PT1H`; the server rejects invalid
    /// or negative text with `invalid_duration` or `negative_duration`.
    Set {
        duration: String,
    },
}

impl WireExpiry {
    /// Expire `duration` after the write is applied.
    #[must_use]
    pub fn after(duration: std::time::Duration) -> Self {
        Self::Set {
            duration: iso_duration(duration),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WireRelease {
    Immediate,
    /// Keep the claim for a fixed ISO 8601 duration after disconnect.
    After {
        duration: String,
    },
}

impl WireRelease {
    /// Keep the claim for `duration` after the session disconnects.
    #[must_use]
    pub fn after(duration: std::time::Duration) -> Self {
        Self::After {
            duration: iso_duration(duration),
        }
    }
}

/// Durations beyond jiff's range saturate; the server then rejects them with
/// `deadline_out_of_range` instead of the client panicking.
fn iso_duration(duration: std::time::Duration) -> String {
    let duration = jiff::SignedDuration::try_from(duration).unwrap_or(jiff::SignedDuration::MAX);
    jiff::fmt::temporal::SpanPrinter::new().duration_to_string(&duration)
}
