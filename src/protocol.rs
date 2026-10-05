use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value as RawJson, json};
use thiserror::Error;

use crate::{
    core::{Change, Snapshot, UpdateBatch},
    domain::{
        ClaimRelease, CommandOccurrence, EventOccurrence, FiniteF64, InputClaim, Node,
        NonNegativeDuration, RetainedValue, Value, WriteProvenance,
    },
};

const TAGS: [&str; 5] = ["$bytes", "$timestamp", "$duration", "$int", "$map"];

#[derive(Clone, Debug, PartialEq)]
pub struct JsonValue(Value);

impl JsonValue {
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn into_inner(self) -> Value {
        self.0
    }
}

impl Serialize for JsonValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        encode(&self.0).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        decode(RawJson::deserialize(deserializer)?)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
struct CodecError {
    message: String,
}

fn error(message: impl Into<String>) -> CodecError {
    CodecError {
        message: message.into(),
    }
}

fn decode(value: RawJson) -> Result<Value, CodecError> {
    match value {
        RawJson::Null => Ok(Value::Null),
        RawJson::Bool(value) => Ok(Value::Bool(value)),
        RawJson::Number(value) if value.is_i64() => Ok(Value::Integer(value.as_i64().unwrap())),
        RawJson::Number(value) if value.is_u64() => Err(error(
            "unsigned JSON integer exceeds the signed runtime range",
        )),
        RawJson::Number(value) => value
            .as_f64()
            .ok_or_else(|| error("invalid JSON number"))
            .and_then(|value| {
                FiniteF64::new(value)
                    .map(Value::Float)
                    .map_err(|source| error(source.to_string()))
            }),
        RawJson::String(value) => Ok(Value::String(value)),
        RawJson::Array(values) => values
            .into_iter()
            .map(decode)
            .collect::<Result<_, _>>()
            .map(Value::List),
        RawJson::Object(mut values) => {
            if values.len() == 1 {
                if let Some(value) = values.remove("$bytes") {
                    return tagged_string(value, "$bytes").and_then(|value| {
                        STANDARD
                            .decode(value)
                            .map(Value::Bytes)
                            .map_err(|source| error(source.to_string()))
                    });
                }
                if let Some(value) = values.remove("$timestamp") {
                    return tagged_string(value, "$timestamp").and_then(|value| {
                        value
                            .parse()
                            .map(Value::Timestamp)
                            .map_err(|source: jiff::Error| error(source.to_string()))
                    });
                }
                if let Some(value) = values.remove("$duration") {
                    return tagged_string(value, "$duration").and_then(|value| {
                        value
                            .parse()
                            .map(Value::Duration)
                            .map_err(|source: jiff::Error| error(source.to_string()))
                    });
                }
                if let Some(value) = values.remove("$int") {
                    return tagged_string(value, "$int").and_then(|value| {
                        value
                            .parse()
                            .map(Value::Integer)
                            .map_err(|source: std::num::ParseIntError| error(source.to_string()))
                    });
                }
                if let Some(value) = values.remove("$map") {
                    let RawJson::Object(values) = value else {
                        return Err(error("$map must contain an object"));
                    };
                    return decode_map(values);
                }
            }
            if let Some(tag) = values.keys().find(|key| TAGS.contains(&key.as_str())) {
                return Err(error(format!(
                    "literal map containing reserved key `{tag}` must use $map"
                )));
            }
            decode_map(values)
        }
    }
}

fn decode_map(values: Map<String, RawJson>) -> Result<Value, CodecError> {
    values
        .into_iter()
        .map(|(key, value)| decode(value).map(|value| (key, value)))
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map(Value::Map)
}

fn tagged_string(value: RawJson, tag: &str) -> Result<String, CodecError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| error(format!("{tag} must contain a string")))
}

#[must_use]
pub fn encode(value: &Value) -> RawJson {
    match value {
        Value::Null => RawJson::Null,
        Value::Bool(value) => RawJson::Bool(*value),
        Value::Integer(value)
            if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(value) =>
        {
            json!(value)
        }
        Value::Integer(value) => json!({"$int": value.to_string()}),
        Value::Float(value) => json!(value.get()),
        Value::String(value) => json!(value),
        Value::Bytes(value) => json!({"$bytes": STANDARD.encode(value)}),
        Value::List(values) => RawJson::Array(values.iter().map(encode).collect()),
        Value::Map(values) => {
            let map: Map<_, _> = values
                .iter()
                .map(|(key, value)| (key.clone(), encode(value)))
                .collect();
            if values.keys().any(|key| TAGS.contains(&key.as_str())) {
                json!({"$map": map})
            } else {
                RawJson::Object(map)
            }
        }
        Value::Timestamp(value) => json!({"$timestamp": value.to_string()}),
        Value::Duration(value) => json!({
            "$duration": jiff::fmt::temporal::SpanPrinter::new().duration_to_string(value)
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotView {
    pub sequence: u64,
    pub nodes: BTreeMap<String, NodeView>,
}

impl From<&Snapshot> for SnapshotView {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            sequence: snapshot.sequence().get(),
            nodes: snapshot
                .nodes()
                .iter()
                .map(|(topic, node)| (topic.to_string(), NodeView::from(node)))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateView {
    pub sequence: u64,
    pub changes: Vec<ChangeView>,
}

impl From<&UpdateBatch> for UpdateView {
    fn from(update: &UpdateBatch) -> Self {
        Self {
            sequence: update.sequence().get(),
            changes: update.changes().iter().map(ChangeView::from).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeView {
    State {
        current: CurrentValueView,
    },
    Event {
        last_publisher: ProvenanceView,
    },
    Desired {
        definition: InputDefinitionView,
        claim: Option<ClaimView>,
        current: Option<CurrentValueView>,
    },
    Command {
        definition: InputDefinitionView,
        claim: Option<ClaimView>,
    },
}

impl From<&Node> for NodeView {
    fn from(node: &Node) -> Self {
        match node {
            Node::State(node) => Self::State {
                current: CurrentValueView::from(node.current()),
            },
            Node::Event(node) => Self::Event {
                last_publisher: ProvenanceView::from(node.last_publisher()),
            },
            Node::Desired(node) => Self::Desired {
                definition: InputDefinitionView {},
                claim: node.claim().map(ClaimView::from),
                current: node.current().map(CurrentValueView::from),
            },
            Node::Command(node) => Self::Command {
                definition: InputDefinitionView {},
                claim: node.claim().map(ClaimView::from),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputDefinitionView {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurrentValueView {
    pub value: JsonValue,
    pub last_write: ProvenanceView,
    pub expires_at: Option<String>,
}

impl From<&RetainedValue> for CurrentValueView {
    fn from(value: &RetainedValue) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
            last_write: ProvenanceView::from(value.last_write()),
            expires_at: value
                .expires_at()
                .map(|deadline| deadline.get().get().to_string()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceView {
    pub client: String,
    pub session: Option<u64>,
    pub at: String,
}

impl From<&WriteProvenance> for ProvenanceView {
    fn from(value: &WriteProvenance) -> Self {
        Self {
            client: value.client().as_str().to_owned(),
            session: value.session().map(|session| session.get()),
            at: value.at().get().to_string(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimView {
    pub id: u64,
    pub owner: String,
    pub session: u64,
    pub release: ClaimReleaseView,
}

impl From<&InputClaim> for ClaimView {
    fn from(value: &InputClaim) -> Self {
        Self {
            id: value.id().get(),
            owner: value.owner().as_str().to_owned(),
            session: value.session().get(),
            release: ClaimReleaseView::from(value.release()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ClaimReleaseView {
    Immediate,
    After { duration: String },
}

impl From<ClaimRelease> for ClaimReleaseView {
    fn from(value: ClaimRelease) -> Self {
        match value {
            ClaimRelease::Immediate => Self::Immediate,
            ClaimRelease::After(duration) => Self::After {
                duration: duration_string(duration),
            },
        }
    }
}

fn duration_string(value: NonNegativeDuration) -> String {
    jiff::fmt::temporal::SpanPrinter::new().duration_to_string(&value.get())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChangeView {
    Upsert {
        topic: String,
        node: NodeView,
    },
    Event {
        topic: String,
        event: OccurrenceView,
    },
    Command {
        topic: String,
        command: OccurrenceView,
    },
    Removed {
        topic: String,
        previous: NodeView,
    },
}

impl From<&Change> for ChangeView {
    fn from(change: &Change) -> Self {
        match change {
            Change::Upsert { topic, node } => Self::Upsert {
                topic: topic.to_string(),
                node: NodeView::from(node),
            },
            Change::Occurrence { topic, event } => Self::Event {
                topic: topic.to_string(),
                event: OccurrenceView::from(event),
            },
            Change::Command { topic, command } => Self::Command {
                topic: topic.to_string(),
                command: OccurrenceView::from(command),
            },
            Change::Removed { topic, previous } => Self::Removed {
                topic: topic.to_string(),
                previous: NodeView::from(previous),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceView {
    pub value: JsonValue,
    pub provenance: ProvenanceView,
}

impl From<&EventOccurrence> for OccurrenceView {
    fn from(value: &EventOccurrence) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
            provenance: ProvenanceView::from(value.publisher()),
        }
    }
}

impl From<&CommandOccurrence> for OccurrenceView {
    fn from(value: &CommandOccurrence) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
            provenance: ProvenanceView::from(value.submitter()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        request_id: RequestId,
        sequence: u64,
        nodes: BTreeMap<String, NodeView>,
    },
    Update {
        sequence: u64,
        changes: Vec<ChangeView>,
    },
    Reply {
        request_id: RequestId,
        result: RawJson,
    },
    Error {
        request_id: Option<RequestId>,
        error: ErrorView,
    },
}

impl ServerMessage {
    #[must_use]
    pub fn snapshot(request_id: RequestId, snapshot: &Snapshot) -> Self {
        let view = SnapshotView::from(snapshot);
        Self::Snapshot {
            request_id,
            sequence: view.sequence,
            nodes: view.nodes,
        }
    }

    #[must_use]
    pub fn update(update: &UpdateBatch) -> Self {
        let view = UpdateView::from(update);
        Self::Update {
            sequence: view.sequence,
            changes: view.changes,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorView {
    pub code: String,
    pub message: String,
}
