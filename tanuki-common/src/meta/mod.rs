use core::fmt::Debug;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

use crate::EntityStatus;

pub trait MetaField: Debug + Clone + Serialize + for<'de> Deserialize<'de> {
    const KEY: CompactString;
}

macro_rules! meta {
    attr(key = $key:expr) (pub struct $ident:ident(pub $($inner:ty)*);) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $ident(pub $($inner)*);

        impl MetaField for $ident {
            const KEY: CompactString = CompactString::const_new($key);
        }
    };
}

#[meta(key = "name")]
pub struct Name(pub CompactString);

#[meta(key = "type")]
pub struct Type(pub CompactString);

#[meta(key = "provider")]
pub struct Provider(pub CompactString);

#[meta(key = "status")]
pub struct Status(pub EntityStatus);

#[meta(key = "version")]
pub struct Version(pub i32);
