use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value as RawJson, json};
use thiserror::Error;

use crate::domain::{FiniteF64, Value};

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
