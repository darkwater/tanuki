use compact_str::CompactString;

use crate::{EntityStatus, property};

pub trait MetaField: crate::Property {}

#[property(MetaField, key = "name")]
pub struct Name(pub CompactString);

#[property(MetaField, key = "type")]
pub struct Type(pub CompactString);

#[property(MetaField, key = "provider")]
pub struct Provider(pub CompactString);

#[property(MetaField, key = "status")]
pub struct Status(pub EntityStatus);

#[property(MetaField, key = "version")]
pub struct Version(pub i32);
