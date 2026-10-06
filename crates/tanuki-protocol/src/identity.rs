use std::{fmt, str::FromStr};

use jiff::SignedDuration;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClientName(String);

impl ClientName {
    pub fn parse(input: &str) -> Result<Self, ClientNameParseError> {
        if input.is_empty() {
            return Err(ClientNameParseError::Empty);
        }
        if input.chars().any(char::is_control) {
            return Err(ClientNameParseError::ControlCharacter);
        }
        if let Some(character) = input
            .chars()
            .find(|character| matches!(character, '/' | '*' | '?' | '[' | ']' | '{' | '}' | '\\'))
        {
            return Err(ClientNameParseError::ReservedCharacter(character));
        }
        Ok(Self(input.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClientName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ClientName {
    type Err = ClientNameParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl Serialize for ClientName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ClientName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        Self::parse(&input).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ClientNameParseError {
    #[error("a client name may not be empty")]
    Empty,
    #[error("a client name may not contain control characters")]
    ControlCharacter,
    #[error("a client name contains reserved character `{0}`")]
    ReservedCharacter(char),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(jiff::Timestamp);

impl Timestamp {
    #[must_use]
    pub const fn new(value: jiff::Timestamp) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> jiff::Timestamp {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Deadline(Timestamp);

impl Deadline {
    #[must_use]
    pub const fn new(value: Timestamp) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> Timestamp {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NonNegativeDuration(SignedDuration);

impl NonNegativeDuration {
    pub fn new(value: SignedDuration) -> Result<Self, NegativeDuration> {
        if value.is_negative() {
            Err(NegativeDuration)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> SignedDuration {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("a timer duration may not be negative")]
pub struct NegativeDuration;
