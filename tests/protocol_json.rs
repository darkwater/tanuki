use std::collections::BTreeMap;

use serde_json::json;
use tanuki::{domain::Value, protocol::JsonValue};

#[test]
fn large_signed_integer_uses_an_exact_tag() {
    let encoded = serde_json::to_value(JsonValue::new(Value::Integer(i64::MAX))).unwrap();
    assert_eq!(encoded, json!({"$int": "9223372036854775807"}));
    let decoded: JsonValue = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.into_inner(), Value::Integer(i64::MAX));
}

#[test]
fn unsigned_integer_outside_runtime_range_is_rejected() {
    let error =
        serde_json::from_value::<JsonValue>(json!(9_223_372_036_854_775_808_u64)).unwrap_err();
    assert!(error.to_string().contains("unsigned JSON integer"));
}

#[test]
fn literal_reserved_keys_are_escaped_without_becoming_semantic_tags() {
    let value = Value::Map(BTreeMap::from([(
        "$bytes".to_owned(),
        Value::String("literal".to_owned()),
    )]));
    let encoded = serde_json::to_value(JsonValue::new(value.clone())).unwrap();
    assert_eq!(encoded, json!({"$map": {"$bytes": "literal"}}));
    let decoded: JsonValue = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.into_inner(), value);
}

#[test]
fn messagepack_uses_native_binary_and_semantic_extensions() {
    let bytes = rmp_serde::to_vec_named(&JsonValue::new(Value::Bytes(vec![0, 1, 2, 255]))).unwrap();
    assert_eq!(bytes, vec![0xc4, 4, 0, 1, 2, 255]);
    let decoded: JsonValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded.into_inner(), Value::Bytes(vec![0, 1, 2, 255]));

    let timestamp: jiff::Timestamp = "2023-11-14T22:13:20Z".parse().unwrap();
    let encoded = rmp_serde::to_vec_named(&JsonValue::new(Value::Timestamp(timestamp))).unwrap();
    assert_eq!(&encoded[..3], &[0xc7, 20, 1]);
    assert!(!encoded.windows(10).any(|window| window == b"$timestamp"));
    let decoded: JsonValue = rmp_serde::from_slice(&encoded).unwrap();
    assert_eq!(decoded.into_inner(), Value::Timestamp(timestamp));

    let duration: jiff::SignedDuration = "PT1H2M3S".parse().unwrap();
    let encoded = rmp_serde::to_vec_named(&JsonValue::new(Value::Duration(duration))).unwrap();
    assert_eq!(&encoded[..2], &[0xd7, 2]);
    let decoded: JsonValue = rmp_serde::from_slice(&encoded).unwrap();
    assert_eq!(decoded.into_inner(), Value::Duration(duration));
}

#[test]
fn json_and_messagepack_round_trip_the_same_nested_value() {
    let value = Value::Map(BTreeMap::from([
        ("null".to_owned(), Value::Null),
        ("integer".to_owned(), Value::Integer(i64::MIN)),
        ("bytes".to_owned(), Value::Bytes(vec![1, 2, 3])),
        (
            "list".to_owned(),
            Value::List(vec![Value::Bool(true), Value::String("tanuki".to_owned())]),
        ),
    ]));
    let json = serde_json::to_vec(&JsonValue::new(value.clone())).unwrap();
    let messagepack = rmp_serde::to_vec_named(&JsonValue::new(value.clone())).unwrap();
    assert_eq!(
        serde_json::from_slice::<JsonValue>(&json)
            .unwrap()
            .into_inner(),
        value
    );
    assert_eq!(
        rmp_serde::from_slice::<JsonValue>(&messagepack)
            .unwrap()
            .into_inner(),
        value
    );
}
