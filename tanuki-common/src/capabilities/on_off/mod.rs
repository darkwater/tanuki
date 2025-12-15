//! Something that can be turned on and off
//!
//! There are exactly two topics; one for state, and one for commands.
//!
//! # Example Entity
//!
//! ```plain
//! ../tanuki.on_off/$meta/version => 1
//! ../tanuki.on_off/on            -> true
//! ../tanuki.on_off/command       <- "on" | "off" | "toggle"
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnOffCommand {
    On,
    Off,
    Toggle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_off_command_format() {
        assert_eq!(serde_json::to_value(&OnOffCommand::On).unwrap(), serde_json::json!("on"));
        assert_eq!(serde_json::to_value(&OnOffCommand::Off).unwrap(), serde_json::json!("off"));
        assert_eq!(
            serde_json::to_value(&OnOffCommand::Toggle).unwrap(),
            serde_json::json!("toggle")
        );
    }
}
