use std::collections::BTreeMap;

use jiff::Timestamp as JiffTimestamp;
use tanuki::domain::{
    ClientName, CommandNode, DesiredNode, EventNode, FiniteF64, InputDefinition, Node,
    RetainedValue, StateNode, Timestamp, Value, ValueKind, WriteContext, WriteProvenance,
};

fn provenance() -> WriteProvenance {
    let client = ClientName::parse("test publisher").unwrap();
    WriteProvenance::from_context(
        &WriteContext::stateless(client),
        Timestamp::new(JiffTimestamp::UNIX_EPOCH),
    )
}

fn retained(value: Value) -> RetainedValue {
    RetainedValue::new(value, provenance(), None)
}

#[test]
fn runtime_float_rejects_non_finite_values() {
    assert!(FiniteF64::new(1.25).is_ok());
    assert!(FiniteF64::new(f64::NAN).is_err());
    assert!(FiniteF64::new(f64::INFINITY).is_err());
    assert!(FiniteF64::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn missing_desired_payload_differs_from_submitted_null() {
    let missing = Node::Desired(DesiredNode::new(InputDefinition::new()));
    let explicit_null = Node::Desired(DesiredNode::with_current(
        InputDefinition::new(),
        retained(Value::Null),
    ));

    assert_eq!(missing.retained_value(), None);
    assert_eq!(
        explicit_null.retained_value().map(RetainedValue::value),
        Some(&Value::Null)
    );
}

#[test]
fn instant_nodes_cannot_retain_payloads_or_expiry() {
    let event = Node::Event(EventNode::new(provenance()));
    let command = Node::Command(CommandNode::new(InputDefinition::new()));

    assert_eq!(event.retained_value(), None);
    assert_eq!(command.retained_value(), None);
}

#[test]
fn a_map_is_one_runtime_value() {
    let value = Value::Map(BTreeMap::from([(
        "clipboard".to_owned(),
        Value::Bytes(vec![1, 2, 3]),
    )]));
    let state = Node::State(StateNode::new(retained(value)));

    assert_eq!(
        state.retained_value().unwrap().value().kind(),
        ValueKind::Map
    );
}

#[test]
fn client_name_deserialization_cannot_bypass_validation() {
    assert!(serde_json::from_str::<ClientName>(r#""phone tasker""#).is_ok());
    assert!(serde_json::from_str::<ClientName>(r#""phone/tasker""#).is_err());
    assert!(serde_json::from_str::<ClientName>(r#""phone*""#).is_err());
}
