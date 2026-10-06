use std::collections::BTreeMap;

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FiniteF64(f64);

impl FiniteF64 {
    pub fn new(value: f64) -> Result<Self, InvalidFloat> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(InvalidFloat)
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for FiniteF64 {
    type Error = InvalidFloat;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq)]
#[error("runtime floats must be finite")]
pub struct InvalidFloat;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(FiniteF64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<Self>),
    Map(BTreeMap<String, Self>),
    Timestamp(jiff::Timestamp),
    Duration(jiff::SignedDuration),
}

impl Value {
    #[must_use]
    pub const fn kind(&self) -> ValueKind {
        match self {
            Self::Null => ValueKind::Null,
            Self::Bool(_) => ValueKind::Bool,
            Self::Integer(_) => ValueKind::Integer,
            Self::Float(_) => ValueKind::Float,
            Self::String(_) => ValueKind::String,
            Self::Bytes(_) => ValueKind::Bytes,
            Self::List(_) => ValueKind::List,
            Self::Map(_) => ValueKind::Map,
            Self::Timestamp(_) => ValueKind::Timestamp,
            Self::Duration(_) => ValueKind::Duration,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ValueKind {
    Null,
    Bool,
    Integer,
    Float,
    String,
    Bytes,
    List,
    Map,
    Timestamp,
    Duration,
}
