use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::{Map, Value as RawJson, json};
use thiserror::Error;

use crate::{ClientName, ErrorCode, FiniteF64, FromValue, PayloadError, TopicPath, Value};

const TAGS: [&str; 3] = ["$bytes", "$int", "$map"];

/// Decode exactly one complete MessagePack value from a transport message.
/// Trailing bytes are an error, just as they are for JSON message decoding.
pub fn decode_messagepack<T: de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, rmp_serde::decode::Error> {
    let mut decoder = rmp_serde::Deserializer::new(bytes);
    let value = T::deserialize(&mut decoder)?;
    if !decoder.get_ref().is_empty() {
        return Err(rmp_serde::decode::Error::Syntax(
            "trailing bytes after MessagePack message".into(),
        ));
    }
    Ok(value)
}

/// A runtime value with Tanuki's codec-aware encoding: tags (`$bytes`, `$int`,
/// `$map`) for JSON and native types for MessagePack.
#[derive(Clone, Debug, PartialEq)]
pub struct WireValue(Value);

impl WireValue {
    /// Decodes into a Serde type or [`Value`]; see [`FromValue`].
    pub fn decode<T: FromValue>(&self) -> Result<T, PayloadError> {
        T::from_value(&self.0)
    }

    #[must_use]
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_inner(&self) -> &Value {
        &self.0
    }

    pub fn into_inner(self) -> Value {
        self.0
    }
}

impl Serialize for WireValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            encode(&self.0).serialize(serializer)
        } else {
            BinaryValue(&self.0).serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for WireValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            decode(RawJson::deserialize(deserializer)?)
                .map(Self)
                .map_err(de::Error::custom)
        } else {
            BinaryOwnedValue::deserialize(deserializer).map(|value| Self(value.0))
        }
    }
}

struct BinaryValue<'a>(&'a Value);

impl Serialize for BinaryValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(value) => serializer.serialize_bool(*value),
            Value::Integer(value) => serializer.serialize_i64(*value),
            Value::Float(value) => serializer.serialize_f64(value.get()),
            Value::String(value) => serializer.serialize_str(value),
            Value::Bytes(value) => serializer.serialize_bytes(value),
            Value::List(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&BinaryValue(value))?;
                }
                sequence.end()
            }
            Value::Map(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, &BinaryValue(value))?;
                }
                map.end()
            }
        }
    }
}

struct BinaryOwnedValue(Value);

impl<'de> Deserialize<'de> for BinaryOwnedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(BinaryValueVisitor)
    }
}

struct BinaryValueVisitor;

impl<'de> Visitor<'de> for BinaryValueVisitor {
    type Value = BinaryOwnedValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a Tanuki MessagePack runtime value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(BinaryOwnedValue(Value::Null))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(BinaryOwnedValue(Value::Bool(value)))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(BinaryOwnedValue(Value::Integer(value)))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        i64::try_from(value)
            .map(Value::Integer)
            .map(BinaryOwnedValue)
            .map_err(|_| E::custom("unsigned MessagePack integer exceeds signed 64-bit range"))
    }

    fn visit_f32<E: de::Error>(self, value: f32) -> Result<Self::Value, E> {
        self.visit_f64(f64::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        FiniteF64::new(value)
            .map(Value::Float)
            .map(BinaryOwnedValue)
            .map_err(E::custom)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(BinaryOwnedValue(Value::String(value)))
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        self.visit_byte_buf(value.to_vec())
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(BinaryOwnedValue(Value::Bytes(value)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        // MessagePack's array header is untrusted and may advertise billions
        // of absent elements in a tiny frame. Allocate only as elements arrive.
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<BinaryOwnedValue>()? {
            values.push(value.0);
        }
        Ok(BinaryOwnedValue(Value::List(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, BinaryOwnedValue>()? {
            values.insert(key, value.0);
        }
        Ok(BinaryOwnedValue(Value::Map(values)))
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        _deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        // rmp-serde presents extension types as a newtype struct.
        Err(de::Error::custom(
            "MessagePack extension types are not Tanuki runtime values",
        ))
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
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotView {
    pub sequence: u64,
    pub nodes: BTreeMap<TopicPath, NodeView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateView {
    pub sequence: u64,
    pub changes: Vec<ChangeView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeView {
    State {
        current: CurrentValueView,
    },
    Event {
        last_publisher: ProvenanceView,
    },
    Desired {
        claim: Option<ClaimView>,
        current: Option<CurrentValueView>,
    },
    Command {
        claim: Option<ClaimView>,
    },
}

/// The four node kinds: retained or instant, output or input.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    State,
    Event,
    Desired,
    Command,
}

impl NodeView {
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        match self {
            Self::State { .. } => NodeKind::State,
            Self::Event { .. } => NodeKind::Event,
            Self::Desired { .. } => NodeKind::Desired,
            Self::Command { .. } => NodeKind::Command,
        }
    }

    /// The retained value of a state node, or a desired node's current
    /// request. `None` for instant nodes and for a desired node without one.
    #[must_use]
    pub const fn current(&self) -> Option<&CurrentValueView> {
        match self {
            Self::State { current } => Some(current),
            Self::Desired { current, .. } => current.as_ref(),
            Self::Event { .. } | Self::Command { .. } => None,
        }
    }

    /// The input claim of a desired or command node.
    #[must_use]
    pub const fn claim(&self) -> Option<&ClaimView> {
        match self {
            Self::Desired { claim, .. } | Self::Command { claim, .. } => claim.as_ref(),
            Self::State { .. } | Self::Event { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurrentValueView {
    pub value: WireValue,
    pub last_write: ProvenanceView,
    pub expires_at: Option<jiff::Timestamp>,
}

impl CurrentValueView {
    /// Decodes the value into a Serde type or [`Value`]; see [`FromValue`].
    pub fn decode<T: FromValue>(&self) -> Result<T, PayloadError> {
        self.value.decode()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceView {
    pub client: ClientName,
    /// The managed session that wrote, absent for stateless writes.
    pub session: Option<u64>,
    pub at: jiff::Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimView {
    pub id: u64,
    pub owner: ClientName,
    pub session: u64,
    pub release: ClaimReleaseView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ClaimReleaseView {
    Immediate,
    After { duration: jiff::SignedDuration },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChangeView {
    Upsert {
        topic: TopicPath,
        node: NodeView,
    },
    Event {
        topic: TopicPath,
        event: OccurrenceView,
    },
    Command {
        topic: TopicPath,
        command: OccurrenceView,
    },
    Removed {
        topic: TopicPath,
        previous: NodeView,
    },
}

impl ChangeView {
    #[must_use]
    pub const fn topic(&self) -> &TopicPath {
        match self {
            Self::Upsert { topic, .. }
            | Self::Event { topic, .. }
            | Self::Command { topic, .. }
            | Self::Removed { topic, .. } => topic,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceView {
    pub value: WireValue,
    pub provenance: ProvenanceView,
}

impl OccurrenceView {
    /// Decodes the payload into a Serde type or [`Value`]; see [`FromValue`].
    pub fn decode<T: FromValue>(&self) -> Result<T, PayloadError> {
        self.value.decode()
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

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for RequestId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// An accepted write: the commit's global sequence and any warnings. It does
/// not promise that a device acted or that the commit was persisted. The
/// same shape is the `data` of a successful HTTP write.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitView {
    pub sequence: u64,
    pub warnings: Vec<DiagnosticView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        request_id: RequestId,
        sequence: u64,
        nodes: BTreeMap<TopicPath, NodeView>,
        warnings: Vec<DiagnosticView>,
    },
    Update {
        sequence: u64,
        changes: Vec<ChangeView>,
    },
    /// The answer to a successful `write`.
    Reply {
        request_id: RequestId,
        result: CommitView,
    },
    Error {
        request_id: Option<RequestId>,
        error: ErrorView,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorView {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DiagnosticView {
    OutputOwnerChanged {
        topic: TopicPath,
        previous: ClientName,
        replacement: ClientName,
    },
    NodeKindChanged {
        topic: TopicPath,
        previous: NodeKind,
        replacement: NodeKind,
    },
    NodeAlreadyAbsent {
        topic: TopicPath,
    },
    SessionReplaced {
        client: ClientName,
        previous: u64,
        replacement: u64,
    },
    InputClaimReplaced {
        topic: TopicPath,
        previous: ClientName,
        replacement: ClientName,
    },
    ClaimGraceOutOfRange {
        topic: TopicPath,
    },
    SchemaWarning {
        topic: TopicPath,
        message: String,
    },
    LinkDisabled {
        link: String,
        topic: TopicPath,
        message: String,
    },
    LinkEnabled {
        link: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_array_length_does_not_allocate_before_reading_elements() {
        struct TruncatedArray;
        impl<'de> SeqAccess<'de> for TruncatedArray {
            type Error = de::value::Error;

            fn size_hint(&self) -> Option<usize> {
                Some(usize::MAX)
            }

            fn next_element_seed<T: de::DeserializeSeed<'de>>(
                &mut self,
                _: T,
            ) -> Result<Option<T::Value>, Self::Error> {
                Err(de::Error::custom("truncated array"))
            }
        }

        // A hostile length must produce the decoder error, not a capacity panic
        // or an allocation proportional to the advertised (but absent) payload.
        assert!(BinaryValueVisitor.visit_seq(TruncatedArray).is_err());
    }
}
