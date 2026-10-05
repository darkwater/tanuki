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
