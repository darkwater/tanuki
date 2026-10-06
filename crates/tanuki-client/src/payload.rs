use crate::protocol::{InvalidFloat, Value};
use serde::{Serialize, de::DeserializeOwned};
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("Serde serialization failed: {0}")]
    Serialize(#[from] serde_value::SerializerError),
    #[error("Serde deserialization failed: {0}")]
    Deserialize(#[from] serde_value::DeserializerError),
    #[error("unsigned integer exceeds signed 64-bit runtime range")]
    IntegerOverflow,
    #[error(transparent)]
    Float(#[from] InvalidFloat),
    #[error("map keys must be strings")]
    MapKey,
    #[error("semantic timestamps/durations require explicit Value access")]
    SemanticValue,
}
/// Convert ordinary Serde data, without interpreting any Tanuki wire tags.
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, PayloadError> {
    narrow(serde_value::to_value(value)?)
}
pub fn from_value<T: DeserializeOwned>(value: &Value) -> Result<T, PayloadError> {
    widen(value)?.deserialize_into().map_err(Into::into)
}
fn narrow(value: serde_value::Value) -> Result<Value, PayloadError> {
    use serde_value::Value as S;
    Ok(match value {
        S::Unit | S::Option(None) => Value::Null,
        S::Bool(v) => Value::Bool(v),
        S::I8(v) => Value::Integer(v.into()),
        S::I16(v) => Value::Integer(v.into()),
        S::I32(v) => Value::Integer(v.into()),
        S::I64(v) => Value::Integer(v),
        S::U8(v) => Value::Integer(v.into()),
        S::U16(v) => Value::Integer(v.into()),
        S::U32(v) => Value::Integer(v.into()),
        S::U64(v) => Value::Integer(v.try_into().map_err(|_| PayloadError::IntegerOverflow)?),
        S::F32(v) => Value::Float(crate::protocol::FiniteF64::new(v.into())?),
        S::F64(v) => Value::Float(crate::protocol::FiniteF64::new(v)?),
        S::String(v) => Value::String(v),
        S::Char(v) => Value::String(v.to_string()),
        S::Bytes(v) => Value::Bytes(v),
        S::Option(Some(v)) | S::Newtype(v) => narrow(*v)?,
        S::Seq(v) => Value::List(v.into_iter().map(narrow).collect::<Result<_, _>>()?),
        S::Map(v) => Value::Map(
            v.into_iter()
                .map(|(key, v)| {
                    let S::String(key) = key else {
                        return Err(PayloadError::MapKey);
                    };
                    Ok((key, narrow(v)?))
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}
fn widen(value: &Value) -> Result<serde_value::Value, PayloadError> {
    use serde_value::Value as S;
    Ok(match value {
        Value::Null => S::Unit,
        Value::Bool(v) => S::Bool(*v),
        Value::Integer(v) => S::I64(*v),
        Value::Float(v) => S::F64(v.get()),
        Value::String(v) => S::String(v.clone()),
        Value::Bytes(v) => S::Bytes(v.clone()),
        Value::List(v) => S::Seq(v.iter().map(widen).collect::<Result<_, _>>()?),
        Value::Map(v) => S::Map(
            v.iter()
                .map(|(key, v)| Ok((S::String(key.clone()), widen(v)?)))
                .collect::<Result<_, PayloadError>>()?,
        ),
        Value::Timestamp(_) | Value::Duration(_) => return Err(PayloadError::SemanticValue),
    })
}
