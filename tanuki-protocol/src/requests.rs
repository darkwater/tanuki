use crate::{ClientName, RequestId, Selector, TopicPath, WireValue};
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
        value: WireValue,
        #[serde(default)]
        expiry: Option<WireExpiry>,
    },
    PublishEvent {
        topic: TopicPath,
        value: WireValue,
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
        value: WireValue,
        #[serde(default)]
        expiry: Option<WireExpiry>,
    },
    SubmitCommand {
        topic: TopicPath,
        value: WireValue,
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
    Set { duration: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WireRelease {
    Immediate,
    After { duration: String },
}
