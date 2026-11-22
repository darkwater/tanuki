use std::collections::BTreeMap;

use serde_json::json;
use tanuki::domain::Value;

#[test]
fn large_signed_integer_uses_an_exact_tag() {
    let encoded = serde_json::to_value(Value::Integer(i64::MAX)).unwrap();
    assert_eq!(encoded, json!({"$int": "9223372036854775807"}));
    let decoded: Value = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, Value::Integer(i64::MAX));
}

#[test]
fn unsigned_integer_outside_runtime_range_is_rejected() {
    let error = serde_json::from_value::<Value>(json!(9_223_372_036_854_775_808_u64)).unwrap_err();
    assert!(error.to_string().contains("unsigned JSON integer"));
}

#[test]
fn literal_reserved_keys_are_escaped_without_becoming_semantic_tags() {
    let value = Value::Map(BTreeMap::from([(
        "$bytes".to_owned(),
        Value::String("literal".to_owned()),
    )]));
    let encoded = serde_json::to_value(value.clone()).unwrap();
    assert_eq!(encoded, json!({"$map": {"$bytes": "literal"}}));
    let decoded: Value = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn messagepack_uses_native_binary_and_rejects_extension_types() {
    let bytes = rmp_serde::to_vec_named(&Value::Bytes(vec![0, 1, 2, 255])).unwrap();
    assert_eq!(bytes, vec![0xc4, 4, 0, 1, 2, 255]);
    let decoded: Value = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, Value::Bytes(vec![0, 1, 2, 255]));

    // A standard MessagePack timestamp (extension -1) is not a runtime value.
    let timestamp = [0xd6, 0xff, 0x65, 0x53, 0xf1, 0x00];
    let error = rmp_serde::from_slice::<Value>(&timestamp).unwrap_err();
    assert!(error.to_string().contains("extension types"), "{error}");
}

#[test]
fn former_time_tags_are_ordinary_json_maps() {
    let encoded = json!({"$timestamp": "2023-11-14T22:13:20Z", "$duration": "PT1H"});
    let value = serde_json::from_value::<Value>(encoded.clone()).unwrap();
    assert_eq!(
        value,
        Value::Map(BTreeMap::from([
            ("$duration".into(), Value::String("PT1H".into())),
            (
                "$timestamp".into(),
                Value::String("2023-11-14T22:13:20Z".into())
            ),
        ]))
    );
    assert_eq!(serde_json::to_value(value).unwrap(), encoded);
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
    let json = serde_json::to_vec(&value.clone()).unwrap();
    let messagepack = rmp_serde::to_vec_named(&value.clone()).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&json).unwrap(), value);
    assert_eq!(rmp_serde::from_slice::<Value>(&messagepack).unwrap(), value);
}
