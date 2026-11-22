use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tanuki_protocol::{Value, from_value, to_value};
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

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Reading {
    level: u8,
    extra: Value,
}

fn sample() -> Value {
    Value::Map(BTreeMap::from([
        ("null".into(), Value::Null),
        ("min".into(), Value::Integer(i64::MIN)),
        (
            "float".into(),
            Value::Float(tanuki_protocol::FiniteF64::new(0.5).unwrap()),
        ),
        ("bytes".into(), Value::Bytes(vec![0, 255])),
        (
            "list".into(),
            Value::List(vec![Value::Bool(true), Value::String("tanuki".into())]),
        ),
        ("$bytes".into(), Value::String("literal".into())),
    ]))
}

#[test]
fn value_is_an_ordinary_serde_type_and_converts_exactly() {
    let value = sample();
    assert_eq!(to_value(&value).unwrap(), value);
    assert_eq!(from_value::<Value>(&value).unwrap(), value);

    // Inside a user type the Value keeps its exact shape: bytes stay bytes
    // rather than becoming a literal {"$bytes": ...} map.
    let reading = Reading {
        level: 70,
        extra: Value::Bytes(vec![1, 2]),
    };
    let converted = to_value(&reading).unwrap();
    assert_eq!(
        converted,
        Value::Map(BTreeMap::from([
            ("extra".into(), Value::Bytes(vec![1, 2])),
            ("level".into(), Value::Integer(70)),
        ]))
    );
    assert_eq!(from_value::<Reading>(&converted).unwrap(), reading);
}

#[test]
fn value_uses_the_tanuki_json_form_and_native_messagepack() {
    let value = sample();
    let json = serde_json::to_value(&value).unwrap();
    // A literal "$bytes" key makes the whole map a `$map` escape.
    assert_eq!(json["$map"]["bytes"], serde_json::json!({"$bytes": "AP8="}));
    assert_eq!(json["$map"]["$bytes"], "literal", "{json}");
    assert_eq!(serde_json::from_value::<Value>(json).unwrap(), value);
    assert_eq!(
        serde_json::to_value(Value::Integer(i64::MAX)).unwrap(),
        serde_json::json!({"$int": "9223372036854775807"})
    );

    let packed = rmp_serde::to_vec_named(&Value::Bytes(vec![1, 2])).unwrap();
    assert_eq!(packed, [0xc4, 2, 1, 2]);
    let packed = rmp_serde::to_vec_named(&value).unwrap();
    assert_eq!(rmp_serde::from_slice::<Value>(&packed).unwrap(), value);
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Shapes {
    blob: Vec<u8>,
    rgb: [u8; 3],
    wide: Vec<u16>,
    empty: Vec<u8>,
}

#[test]
fn u8_sequences_are_bytes_but_fixed_arrays_and_other_lists_are_lists() {
    let shapes = Shapes {
        blob: vec![0, 1, 255],
        rgb: [255, 0, 0],
        wide: vec![1, 2],
        empty: vec![],
    };
    let value = to_value(&shapes).unwrap();
    let Value::Map(fields) = &value else {
        panic!("a struct is a map")
    };
    assert_eq!(fields["blob"], Value::Bytes(vec![0, 1, 255]));
    let list = |items: &[i64]| Value::List(items.iter().copied().map(Value::Integer).collect());
    assert_eq!(fields["rgb"], list(&[255, 0, 0]));
    assert_eq!(fields["wide"], list(&[1, 2]));
    // No elements: nothing says these were bytes.
    assert_eq!(fields["empty"], Value::List(vec![]));
    assert_eq!(from_value::<Shapes>(&value).unwrap(), shapes);

    // Reading is lenient both ways.
    assert_eq!(from_value::<Vec<u8>>(&list(&[7, 8])).unwrap(), [7, 8]);
    assert_eq!(
        from_value::<[u8; 2]>(&Value::Bytes(vec![7, 8])).unwrap(),
        [7, 8]
    );
    assert!(from_value::<Vec<u8>>(&list(&[256])).is_err());
}
