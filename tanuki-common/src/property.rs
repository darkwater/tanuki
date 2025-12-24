use core::fmt::Debug;

use serde::{Deserialize, Serialize};

pub enum PropertyKind {
    /// Persistent state from the entity (eg. sensor readings)
    State,
    /// Transient updates from the entity (eg. button presses)
    Event,
    /// Commands sent to the entity (eg. turn on/off)
    Command,
}

pub trait Property: Debug + Clone + Serialize + for<'de> Deserialize<'de> {
    const KEY: &str;
    const KIND: PropertyKind;
}
