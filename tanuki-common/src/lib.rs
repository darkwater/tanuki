#![cfg_attr(not(test), no_std)]
#![feature(macro_attr)]

extern crate alloc;

use core::fmt::Display;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

pub mod capabilities;
pub mod meta;

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
        entity: CompactString,
        key: CompactString,
    },
    CapabilityMeta {
        entity: CompactString,
        capability: CompactString,
        key: CompactString,
    },
    CapabilityData {
        entity: CompactString,
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
