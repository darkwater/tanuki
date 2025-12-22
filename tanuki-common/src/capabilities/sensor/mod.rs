//! Simple numeric/boolean sensor data with unit and timestamp
//!
//! Topics map to what is measured (eg. `temperature`), and the payload is a [`SensorPayload`].
//!
//! # Example Entity
//!
//! ```plain
//! ../tanuki.sensor/$meta/version => 1
//! ../tanuki.sensor/temperature   => { value: 23.5, unit: "°C", timestamp: 1712345678 }
//! ../tanuki.sensor/humidity      => { value: 45.0, unit: "%",  timestamp: 1712345678 }
//! ../tanuki.sensor/motion        => { value: true, unit: "",   timestamp: 1712345678 }
//! ../tanuki.sensor/battery       => { value: 82,   unit: "%",  timestamp: 1712345678 }
//! ```

use chrono::{DateTime, Utc};
use compact_str::CompactString;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorPayload {
    /// Value of the sensor
    pub value: SensorValue,
    /// Unit of the sensor value, e.g., "°C", "%", "V"
    pub unit: CompactString,
    /// Unix timestamp in seconds
    #[serde(with = "chrono::serde::ts_seconds")]
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SensorValue {
    Number(f32),
    Boolean(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_payload_format() {
        assert_eq!(
            serde_json::to_value(&SensorPayload {
                value: SensorValue::Number(23.5),
                unit: "°C".into(),
                timestamp: DateTime::<Utc>::from_timestamp_secs(1712345678).unwrap(),
            })
            .unwrap(),
            serde_json::json!({
                "value": 23.5,
                "unit": "°C",
                "timestamp": 1712345678
            })
        );
    }

    #[test]
    fn deser_values() {
        assert_eq!(
            serde_json::from_value::<SensorValue>(serde_json::json!(23.5)).unwrap(),
            SensorValue::Number(23.5)
        );
        assert_eq!(
            serde_json::from_value::<SensorValue>(serde_json::json!(true)).unwrap(),
            SensorValue::Boolean(true)
        );
    }
}
