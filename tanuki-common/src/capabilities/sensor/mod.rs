use compact_str::CompactString;
use mqtt_protocol_core::mqtt::IntoPayload;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorPayload {
    /// Value of the sensor
    pub value: SensorValue,
    /// Unit of the sensor value, e.g., "°C", "%", "V"
    pub unit: CompactString,
    /// Unix timestamp in seconds
    pub timestamp: i64,
}

impl IntoPayload for &SensorPayload {
    fn into_payload(self) -> mqtt_protocol_core::mqtt::ArcPayload {
        serde_json::to_vec(self).unwrap().into_payload()
    }
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
                timestamp: 1712345678,
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
