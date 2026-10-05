use jiff::Timestamp as JiffTimestamp;
use serde_json::json;
use tanuki::{
    core::{Core, SubscriptionCapacity},
    domain::{
        ClientName, ExpiryUpdate, InputDefinition, InputKind, Selection, Selector, Timestamp,
        TopicPath, Value, WriteBatch, WriteContext, WriteOperation,
    },
    protocol::{RequestId, ServerMessage, SnapshotView, UpdateView},
};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn actor() -> WriteContext {
    WriteContext::stateless(ClientName::parse("phone").unwrap())
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

#[test]
fn snapshot_message_is_the_correlated_subscribe_success() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![WriteOperation::PublishState {
            topic: topic("/battery/phone"),
            value: Value::Integer(72),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(1),
    )
    .unwrap();
    let snapshot = core.read(&Selection::new(vec![
        Selector::parse("/battery/*").unwrap(),
    ]));

    let message = ServerMessage::snapshot(RequestId::new("s1".to_owned()), &snapshot);
    let encoded = serde_json::to_value(&message).unwrap();
    assert_eq!(encoded["type"], "snapshot");
    assert_eq!(encoded["request_id"], "s1");
    assert_eq!(encoded["sequence"], 1);
    assert_eq!(encoded["nodes"]["/battery/phone"]["current"]["value"], 72);
    assert_eq!(
        serde_json::from_value::<ServerMessage>(encoded).unwrap(),
        message
    );
}

#[test]
fn desired_missing_and_submitted_null_have_distinct_node_views() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![
            WriteOperation::DefineInput {
                topic: topic("/request/missing"),
                kind: InputKind::Desired,
                definition: InputDefinition::new(),
            },
            WriteOperation::DefineInput {
                topic: topic("/request/null"),
                kind: InputKind::Desired,
                definition: InputDefinition::new(),
            },
            WriteOperation::SubmitDesired {
                topic: topic("/request/null"),
                value: Value::Null,
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        at(1),
    )
    .unwrap();
    let view = SnapshotView::from(&core.read(&Selection::new(vec![
        Selector::parse("/request/*").unwrap(),
    ])));
    let encoded = serde_json::to_value(view).unwrap();
    assert!(encoded["nodes"]["/request/missing"]["current"].is_null());
    assert!(encoded["nodes"]["/request/null"]["current"].is_object());
    assert!(encoded["nodes"]["/request/null"]["current"]["value"].is_null());
}

#[test]
fn update_message_keeps_one_commit_and_distinct_occurrence_variants() {
    let mut core = Core::new();
    let mut subscription = core
        .subscribe(
            Selection::new(vec![Selector::parse("/**").unwrap()]),
            SubscriptionCapacity::new(4).unwrap(),
        )
        .unwrap();
    core.apply(
        &actor(),
        batch(vec![
            WriteOperation::PublishState {
                topic: topic("/battery/phone"),
                value: Value::Integer(72),
                expiry: ExpiryUpdate::Clear,
            },
            WriteOperation::PublishEvent {
                topic: topic("/doorbell/rang"),
                value: Value::Bool(true),
            },
        ]),
        at(1),
    )
    .unwrap();
    let update = subscription.try_update().unwrap();
    let view = UpdateView::from(&update);
    assert_eq!(view.sequence, 1);
    assert_eq!(view.changes.len(), 3);

    let encoded = serde_json::to_value(ServerMessage::update(&update)).unwrap();
    assert_eq!(encoded["type"], "update");
    assert_eq!(encoded["sequence"], 1);
    assert_eq!(
        encoded["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["upsert", "upsert", "event"]
    );
    assert_eq!(encoded["changes"][2]["event"]["value"], json!(true));
}
