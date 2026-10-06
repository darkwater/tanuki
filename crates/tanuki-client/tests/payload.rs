use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tanuki_client::protocol::Value;
use tanuki_client::{from_value, to_value};
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Payload {
    #[serde(rename = "$bytes")]
    literal: String,
    limit: i64,
    mode: Mode,
    optional: Option<u16>,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum Mode {
    Idle,
    Level(u16),
    Pair(u8, u8),
    Named { hue: u16 },
}
#[test]
fn structs_enum_shapes_bytes_options_and_reserved_keys_round_trip() {
    for mode in [
        Mode::Idle,
        Mode::Level(120),
        Mode::Pair(1, 2),
        Mode::Named { hue: 35 },
    ] {
        let payload = Payload {
            literal: "not base64".into(),
            limit: i64::MAX,
            mode,
            optional: Some(65535),
            bytes: vec![0, 255],
        };
        let value = to_value(&payload).unwrap();
        let Value::Map(ref fields) = value else {
            panic!("struct must be a map")
        };
        assert_eq!(fields["bytes"], Value::Bytes(vec![0, 255]));
        assert_eq!(fields["$bytes"], Value::String("not base64".into()));
        assert_eq!(from_value::<Payload>(&value).unwrap(), payload);
    }
    assert_eq!(to_value(&None::<u16>).unwrap(), Value::Null);
    assert_eq!(from_value::<Option<u16>>(&Value::Null).unwrap(), None);
    assert_eq!(
        from_value::<Option<u16>>(&Value::Integer(70)).unwrap(),
        Some(70)
    );
}
#[test]
fn conversion_rejects_nonfinite_overflow_and_nonstring_keys_without_rounding() {
    assert_eq!(
        to_value(&(i64::MAX as u64)).unwrap(),
        Value::Integer(i64::MAX)
    );
    assert_eq!(to_value(&i64::MIN).unwrap(), Value::Integer(i64::MIN));
    assert!(to_value(&u64::MAX).is_err());
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(to_value(&value).is_err());
    }
    assert!(to_value(&BTreeMap::from([(1u8, "bad")])).is_err());
    assert!(from_value::<u8>(&Value::Integer(256)).is_err());
}
