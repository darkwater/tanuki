#![cfg_attr(not(test), no_std)]
#![feature(macro_attr)]

extern crate alloc;

use core::fmt::Display;

use compact_str::{CompactString, ToCompactString};
use serde::{Deserialize, Serialize};

pub mod capabilities;
pub mod meta;

#[doc(hidden)]
pub use serde as _serde;

mod property;
pub use property::Property;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub CompactString);
impl<T: AsRef<str>> From<T> for EntityId {
    fn from(value: T) -> Self {
        EntityId(value.as_ref().to_compact_string())
    }
}
impl Display for EntityId {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityStatus {
    /// The entity is online but its data may not yet be valid
    Init,
    /// The entity is online and its data is valid
    Online,
    /// The entity disconnected cleanly
    Disconnected,
    /// The entity was unexpectedly disconnected and its data may not be valid
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topic {
    EntityMeta {
        entity: EntityId,
        key: CompactString,
    },
    CapabilityMeta {
        entity: EntityId,
        capability: CompactString,
        key: CompactString,
    },
    CapabilityData {
        entity: EntityId,
        capability: CompactString,
        rest: CompactString,
    },
}

impl Display for Topic {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        match self {
            Topic::EntityMeta { entity, key } => {
                write!(f, "tanuki/entities/{}/$meta/{}", entity, key)
            }
            Topic::CapabilityMeta { entity, capability, key } => {
                write!(f, "tanuki/entities/{}/{}/$meta/{}", entity, capability, key)
            }
            Topic::CapabilityData { entity, capability, rest } => {
                write!(f, "tanuki/entities/{}/{}/{}", entity, capability, rest)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_serde() {
        assert_eq!(
            serde_json::to_string(&EntityId::from("test.entity")).unwrap(),
            r#""test.entity""#
        );

        assert_eq!(
            serde_json::from_str::<EntityId>(r#""test.entity""#).unwrap(),
            EntityId::from("test.entity")
        );
    }
}
