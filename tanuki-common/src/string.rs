use core::{
    fmt::Display,
    ops::{Add, Deref, DerefPure},
};

use compact_str::{CompactString, ToCompactString};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TanukiString(CompactString);

impl TanukiString {
    pub fn new(compact_string: CompactString) -> Self {
        Self(compact_string)
    }

    pub const fn const_new(s: &'static str) -> Self {
        Self(CompactString::const_new(s))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Deref for TanukiString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

unsafe impl DerefPure for TanukiString {}

impl<T: AsRef<str>> From<T> for TanukiString {
    fn from(value: T) -> Self {
        TanukiString(CompactString::from(value.as_ref()))
    }
}

impl<T: AsRef<str>> Add<T> for TanukiString {
    type Output = Self;

    fn add(self, rhs: T) -> Self::Output {
        TanukiString(self.0 + rhs.as_ref())
    }
}

impl<T: AsRef<str> + ?Sized> PartialEq<T> for TanukiString {
    fn eq(&self, other: &T) -> bool {
        self.0.as_str() == other.as_ref()
    }
}

impl Display for TanukiString {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        self.0.fmt(f)
    }
}

pub trait ToTanukiString: ToCompactString {
    fn to_tanuki_string(&self) -> TanukiString {
        TanukiString(self.to_compact_string())
    }
}

impl<T: ToCompactString> ToTanukiString for T {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub TanukiString);

impl EntityId {
    pub const WILDCARD: Self = EntityId(TanukiString::const_new("+"));

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: AsRef<str>> From<T> for EntityId {
    fn from(value: T) -> Self {
        EntityId(value.as_ref().to_tanuki_string())
    }
}

impl Deref for EntityId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

unsafe impl DerefPure for EntityId {}

impl Display for EntityId {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", self.0)
    }
}
