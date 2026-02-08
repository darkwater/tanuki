//! Physical buttons that can be pressed
//!
//! Each topic represents a physical button, and sends actions for button presses.
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
pub enum ButtonAction {
    /// Button was pressed
    Pressed,
    /// Button was held down for some time
    LongPressed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_action_format() {
        assert_eq!(
            serde_json::to_value(ButtonAction::Pressed).unwrap(),
            serde_json::json!("pressed")
        );
    }
}
