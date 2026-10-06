use crate::{ClientName, JsonValue, RequestId, Selector, TopicPath};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        request_id: RequestId,
        client: ClientName,
        selectors: Vec<Selector>,
    },
    Write {
        request_id: RequestId,
        operations: Vec<WireOperation>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WireOperation {
    PublishState {
        topic: TopicPath,
        value: JsonValue,
        #[serde(default)]
        expiry: Option<WireExpiry>,
    },
    PublishEvent {
        topic: TopicPath,
        value: JsonValue,
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
        value: JsonValue,
        #[serde(default)]
        expiry: Option<WireExpiry>,
    },
    SubmitCommand {
        topic: TopicPath,
        value: JsonValue,
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
