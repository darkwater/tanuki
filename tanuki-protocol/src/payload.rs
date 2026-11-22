//! Conversion between Serde types and [`Value`].
//!
//! Both directions report themselves as *not* human-readable, so a [`Value`]
//! nested in a Serde type keeps its exact shape; the tagged JSON form
//! (`{"$bytes": ...}`) is used only by human-readable formats such as JSON.
//! Sequences whose elements are all `u8` become [`Value::Bytes`]; fixed-size
//! arrays and tuples (`[u8; 3]`, `(u8, u8)`) stay lists.
mod de;
mod ser;

use crate::{InvalidFloat, Value};
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Display;

#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    /// A type did not match, a field was missing, or a Serde implementation
    /// reported its own error.
    #[error("{0}")]
    Message(String),
    #[error("integer exceeds the signed 64-bit runtime range")]
    IntegerOverflow,
    #[error(transparent)]
    Float(#[from] InvalidFloat),
    #[error("map keys must be strings")]
    MapKey,
}

impl serde::ser::Error for PayloadError {
    fn custom<T: Display>(message: T) -> Self {
        Self::Message(message.to_string())
    }
}

impl serde::de::Error for PayloadError {
    fn custom<T: Display>(message: T) -> Self {
        Self::Message(message.to_string())
    }
}

/// Converts any Serde type, including [`Value`] itself, into a [`Value`].
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, PayloadError> {
    value.serialize(ser::ValueSerializer::root())
}

/// Converts a [`Value`] into any Serde type, including [`Value`] itself.
pub fn from_value<T: DeserializeOwned>(value: &Value) -> Result<T, PayloadError> {
    T::deserialize(de::ValueDeserializer(value))
}
