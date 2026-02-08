#![cfg_attr(not(test), no_std)]
#![feature(deref_pure_trait, macro_attr, macro_derive, str_split_remainder)]

extern crate alloc;

use core::{fmt::Display, str::FromStr};

pub mod capabilities;
pub mod macros;
pub mod meta;

#[doc(hidden)]
pub use serde as _serde;

mod property;
mod string;
pub use property::*;
pub use string::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topic {
    EntityMeta {
        entity: EntityId,
        key: TanukiString,
    },
    CapabilityMeta {
        entity: EntityId,
        capability: TanukiString,
        key: TanukiString,
    },
    CapabilityData {
        entity: EntityId,
        capability: TanukiString,
        rest: TanukiString,
    },
}

impl Topic {
    pub const CAPABILITY_DATA_WILDCARD: Self = Self::CapabilityData {
        entity: EntityId::WILDCARD,
        capability: TanukiString::const_new("+"),
        rest: TanukiString::const_new("#"),
    };
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

impl FromStr for Topic {
    type Err = &'static str;

    fn from_str(s: &str) -> core::result::Result<Self, Self::Err> {
        let mut parts = s.split('/');
        if parts.next() != Some("tanuki") {
            return Err("does not start with tanuki/");
        }

        match parts.next() {
            Some("entities") => match parts.next() {
                Some(entity) => match parts.next() {
                    Some("$meta") => match parts.next() {
                        Some(key) if parts.next().is_none() => Ok(Topic::EntityMeta {
                            entity: EntityId::from(entity),
                            key: key.to_tanuki_string(),
                        }),
                        Some(_) => Err("tanuki/entities/{id}/$meta/{key}/..."),
                        _ => Err("tanuki/entities/{id}/$meta"),
                    },
                    Some(capability) => match parts.next() {
                        Some("$meta") => match parts.next() {
                            Some(key) if parts.next().is_none() => Ok(Topic::CapabilityMeta {
                                entity: EntityId::from(entity),
                                capability: capability.to_tanuki_string(),
                                key: key.to_tanuki_string(),
                            }),
                            Some(_) => Err("tanuki/entities/{id}/{cap}/$meta/{key}/..."),
                            _ => Err("tanuki/entities/{id}/{cap}/$meta"),
                        },
                        Some(rest) => Ok(Topic::CapabilityData {
                            entity: EntityId::from(entity),
                            capability: capability.to_tanuki_string(),
                            rest: match parts.remainder() {
                                Some(remainder) => rest.to_tanuki_string() + "/" + remainder,
                                None => rest.to_tanuki_string(),
                            },
                        }),
                        None => Err("tanuki/entities/{id}/{cap}"),
                    },
                    None => Err("tanuki/entities/{id}"),
                },
                None => Err("tanuki/entities"),
            },
            Some(_) => Err("tanuki/..."),
            None => Err("tanuki"),
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

    #[test]
    fn topic_display() {
        assert_eq!(
            Topic::EntityMeta {
                entity: EntityId::from("sensor.temperature"),
                key: "status".to_tanuki_string(),
            }
            .to_string(),
            "tanuki/entities/sensor.temperature/$meta/status"
        );

        assert_eq!(
            Topic::CapabilityMeta {
                entity: EntityId::from("sensor.temperature"),
                capability: "temperature_sensor".to_tanuki_string(),
                key: "version".to_tanuki_string(),
            }
            .to_string(),
            "tanuki/entities/sensor.temperature/temperature_sensor/$meta/version"
        );

        assert_eq!(
            Topic::CapabilityData {
                entity: EntityId::from("sensor.temperature"),
                capability: "temperature_sensor".to_tanuki_string(),
                rest: "current".to_tanuki_string(),
            }
            .to_string(),
            "tanuki/entities/sensor.temperature/temperature_sensor/current"
        );
    }

    #[test]
    fn topic_from_str() {
        assert_eq!(
            "tanuki/entities/sensor.temperature/$meta/status"
                .parse::<Topic>()
                .unwrap(),
            Topic::EntityMeta {
                entity: EntityId::from("sensor.temperature"),
                key: "status".to_tanuki_string(),
            }
        );

        assert_eq!(
            "tanuki/entities/sensor.temperature/$meta/status/extra".parse::<Topic>(),
            Err("tanuki/entities/{id}/$meta/{key}/...")
        );

        assert_eq!(
            "tanuki/entities/sensor.temperature/temperature_sensor/$meta/version"
                .parse::<Topic>()
                .unwrap(),
            Topic::CapabilityMeta {
                entity: EntityId::from("sensor.temperature"),
                capability: "temperature_sensor".to_tanuki_string(),
                key: "version".to_tanuki_string(),
            }
        );

        assert_eq!(
            "tanuki/entities/sensor.temperature/temperature_sensor/$meta/version/extra"
                .parse::<Topic>(),
            Err("tanuki/entities/{id}/{cap}/$meta/{key}/...")
        );

        assert_eq!(
            "tanuki/entities/sensor.temperature/temperature_sensor/current"
                .parse::<Topic>()
                .unwrap(),
            Topic::CapabilityData {
                entity: EntityId::from("sensor.temperature"),
                capability: "temperature_sensor".to_tanuki_string(),
                rest: "current".to_tanuki_string(),
            }
        );

        assert_eq!(
            "tanuki/entities/sensor.temperature/temperature_sensor/current/extra"
                .parse::<Topic>()
                .unwrap(),
            Topic::CapabilityData {
                entity: EntityId::from("sensor.temperature"),
                capability: "temperature_sensor".to_tanuki_string(),
                rest: "current/extra".to_tanuki_string(),
            }
        );
    }
}
