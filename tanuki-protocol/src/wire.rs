use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::{SerializeMap, SerializeSeq, SerializeTuple},
};
use serde_json::{Map, Value as RawJson, json};
use thiserror::Error;

use crate::{FiniteF64, Value};

const TAGS: [&str; 5] = ["$bytes", "$timestamp", "$duration", "$int", "$map"];

#[derive(Clone, Debug, PartialEq)]
pub struct JsonValue(Value);

impl JsonValue {
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

impl Serialize for JsonValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            encode(&self.0).serialize(serializer)
        } else {
            BinaryValue(&self.0).serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for JsonValue {
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
            Value::Timestamp(value) => ExtValue {
                tag: -1,
                bytes: &encode_timestamp(*value),
            }
            .serialize(serializer),
            Value::Duration(value) => {
                let text = jiff::fmt::temporal::SpanPrinter::new().duration_to_string(value);
                ExtValue {
                    tag: 2,
                    bytes: text.as_bytes(),
                }
                .serialize(serializer)
            }
        }
    }
}

struct ExtValue<'a> {
    tag: i8,
    bytes: &'a [u8],
}

impl Serialize for ExtValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct("_ExtStruct", &ExtPayload(self.tag, self.bytes))
    }
}

struct ExtPayload<'a>(i8, &'a [u8]);

impl Serialize for ExtPayload<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(&self.0)?;
        tuple.serialize_element(&BinaryBytes(self.1))?;
        tuple.end()
    }
}

struct BinaryBytes<'a>(&'a [u8]);

impl Serialize for BinaryBytes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
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
        let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
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
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        let ExtOwnedPayload(tag, bytes) = ExtOwnedPayload::deserialize(deserializer)?;
        match tag {
            -1 => decode_timestamp(&bytes.0)
                .map(Value::Timestamp)
                .map(BinaryOwnedValue)
                .map_err(de::Error::custom),
            2 => String::from_utf8(bytes.0)
                .map_err(de::Error::custom)?
                .parse()
                .map(Value::Duration)
                .map(BinaryOwnedValue)
                .map_err(de::Error::custom),
            _ => Err(de::Error::custom(format!(
                "unknown Tanuki MessagePack extension tag {tag}"
            ))),
        }
    }
}

fn encode_timestamp(value: jiff::Timestamp) -> Vec<u8> {
    let mut seconds = value.as_second();
    let subsecond = value.subsec_nanosecond();
    let nanoseconds = if subsecond < 0 {
        seconds -= 1;
        (1_000_000_000 + subsecond) as u32
    } else {
        subsecond as u32
    };

    if (0..(1_i64 << 34)).contains(&seconds) {
        let packed = (u64::from(nanoseconds) << 34) | seconds as u64;
        if packed <= u64::from(u32::MAX) {
            return (packed as u32).to_be_bytes().to_vec();
        }
        return packed.to_be_bytes().to_vec();
    }

    let mut bytes = Vec::with_capacity(12);
    bytes.extend_from_slice(&nanoseconds.to_be_bytes());
    bytes.extend_from_slice(&seconds.to_be_bytes());
    bytes
}

fn decode_timestamp(bytes: &[u8]) -> Result<jiff::Timestamp, CodecError> {
    let (seconds, nanoseconds) = match bytes {
        [a, b, c, d] => (i64::from(u32::from_be_bytes([*a, *b, *c, *d])), 0),
        [a, b, c, d, e, f, g, h] => {
            let packed = u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h]);
            (
                (packed & 0x0000_0003_ffff_ffff) as i64,
                (packed >> 34) as i32,
            )
        }
        [a, b, c, d, e, f, g, h, i, j, k, l] => (
            i64::from_be_bytes([*e, *f, *g, *h, *i, *j, *k, *l]),
            u32::from_be_bytes([*a, *b, *c, *d]) as i32,
        ),
        _ => {
            return Err(error(
                "MessagePack timestamp payload must be 4, 8, or 12 bytes",
            ));
        }
    };
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(error("MessagePack timestamp nanoseconds are out of range"));
    }
    jiff::Timestamp::new(seconds, nanoseconds).map_err(|source| error(source.to_string()))
}

#[derive(Deserialize)]
struct ExtOwnedPayload(i8, OwnedBytes);

struct OwnedBytes(Vec<u8>);

impl<'de> Deserialize<'de> for OwnedBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BytesVisitor;

        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = OwnedBytes;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("MessagePack binary data")
            }

            fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(OwnedBytes(value.to_vec()))
            }

            fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                Ok(OwnedBytes(value))
            }
        }

        deserializer.deserialize_byte_buf(BytesVisitor)
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
        definition: InputDefinitionView,
        claim: Option<ClaimView>,
        current: Option<CurrentValueView>,
    },
    Command {
        definition: InputDefinitionView,
        claim: Option<ClaimView>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputDefinitionView {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurrentValueView {
    pub value: JsonValue,
    pub last_write: ProvenanceView,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceView {
    pub client: String,
    pub session: Option<u64>,
    pub at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimView {
    pub id: u64,
    pub owner: String,
    pub session: u64,
    pub release: ClaimReleaseView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ClaimReleaseView {
    Immediate,
    After { duration: String },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceView {
    pub value: JsonValue,
    pub provenance: ProvenanceView,
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

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        request_id: RequestId,
        sequence: u64,
        nodes: BTreeMap<String, NodeView>,
        warnings: Vec<DiagnosticView>,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorView {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DiagnosticView {
    OutputOwnerChanged {
        topic: String,
        previous: String,
        replacement: String,
    },
    NodeKindChanged {
        topic: String,
        previous: String,
        replacement: String,
    },
    NodeAlreadyAbsent {
        topic: String,
    },
    SessionReplaced {
        client: String,
        previous: u64,
        replacement: u64,
    },
    InputClaimReplaced {
        topic: String,
        previous: String,
        replacement: String,
    },
    ClaimGraceOutOfRange {
        topic: String,
    },
    SchemaWarning {
        topic: String,
        message: String,
    },
    LinkDisabled {
        link: String,
        topic: String,
        message: String,
    },
    LinkEnabled {
        link: String,
    },
}
