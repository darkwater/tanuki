//! Physical buttons that can be pressed
//!
//! Each topic represents a physical button, and sends events for button presses.
//!
//! # Example Entity
//!
//! ```plain
//! ../tanuki.buttons/$meta/version => 1
//! ../tanuki.buttons/on            -> "pressed"
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonEvent {
    /// Button was pressed
    Pressed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_event_format() {
        assert_eq!(
            serde_json::to_value(ButtonEvent::Pressed).unwrap(),
            serde_json::json!("pressed")
        );
    }
}
