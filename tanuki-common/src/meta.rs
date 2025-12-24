use compact_str::CompactString;

use crate::property;

pub trait MetaField: crate::Property {}

#[property(MetaField, State, key = "name")]
pub struct Name(pub CompactString);

#[property(MetaField, State, key = "type")]
pub struct Type(pub CompactString);

#[property(MetaField, State, key = "provider")]
pub struct Provider(pub CompactString);

#[property(MetaField, State, key = "status")]
#[derive(Copy, Eq)]
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

#[property(MetaField, State, key = "version")]
pub struct Version(pub i32);
