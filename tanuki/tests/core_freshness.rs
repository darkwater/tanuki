use jiff::{SignedDuration, Timestamp as JiffTimestamp};
use tanuki::{
    core::{Change, Core, SchemaInstallMode, SubscriptionCapacity, SubscriptionEnd},
    domain::{
        ClientName, ExpiryUpdate, InputDefinition, InputKind, NonNegativeDuration, Selection,
        Selector, Timestamp, TopicPath, Value, WriteBatch, WriteContext, WriteOperation,
    },
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueValidator},
};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn seconds(value: i64) -> NonNegativeDuration {
    NonNegativeDuration::new(SignedDuration::from_secs(value)).unwrap()
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn actor() -> WriteContext {
    WriteContext::stateless(ClientName::parse("sensor").unwrap())
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn freshness_schema() -> Schema {
    Schema::new(
        SchemaName::parse("freshness").unwrap(),
        vec![
            SchemaRule::new(
                Selector::parse("/tracked/**").unwrap(),
                Enforcement::Warn,
                ValueValidator::any(NullPolicy::Allow),
            )
            .unwrap()
            .with_expected_update_interval(seconds(10))
            .unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn overdue_state_is_retained_and_duplicate_write_clears_the_active_condition() {
    let mut core = Core::new();
    core.install_schema(freshness_schema(), SchemaInstallMode::RejectInvalid, at(0))
        .unwrap();
    core.apply(
        &actor(),
        batch(vec![WriteOperation::PublishState {
            topic: topic("/tracked/temperature"),
            value: Value::Integer(21),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(0),
    )
    .unwrap();
    assert_eq!(core.next_value_deadline().unwrap().get(), at(10));
    assert!(core.process_deadlines(at(9), &[]).unwrap().is_none());

    let overdue = core.process_deadlines(at(10), &[]).unwrap().unwrap();
    let condition = topic("/$diagnostics/freshness/tracked/temperature");
    assert!(overdue.changes().iter().any(|change| matches!(
        change,
        Change::Upsert { topic, .. } if topic == &condition
    )));
    let all = core.read(&Selection::new(vec![Selector::parse("/**").unwrap()]));
    assert!(all.get(&topic("/tracked/temperature")).is_some());
    assert!(all.get(&condition).is_some());
    assert!(core.next_value_deadline().is_none());

    let refreshed = core
        .apply(
            &actor(),
            batch(vec![WriteOperation::PublishState {
                topic: topic("/tracked/temperature"),
                value: Value::Integer(21),
                expiry: ExpiryUpdate::Clear,
            }]),
            at(11),
        )
        .unwrap();
    assert!(refreshed.update().changes().iter().any(|change| matches!(
        change,
        Change::Removed { topic, .. } if topic == &condition
    )));
    assert!(
        core.read(&Selection::new(vec![
            Selector::parse("/$diagnostics/**").unwrap()
        ]))
        .nodes()
        .is_empty()
    );
    assert_eq!(core.next_value_deadline().unwrap().get(), at(21));
}

#[test]
fn definition_only_desired_and_instant_nodes_do_not_get_freshness_deadlines() {
    let mut core = Core::new();
    core.install_schema(freshness_schema(), SchemaInstallMode::RejectInvalid, at(0))
        .unwrap();
    core.apply(
        &actor(),
        batch(vec![
            WriteOperation::DefineInput {
                topic: topic("/tracked/desired"),
                kind: InputKind::Desired,
                definition: InputDefinition::new(),
            },
            WriteOperation::PublishEvent {
                topic: topic("/tracked/event"),
                value: Value::Bool(true),
            },
        ]),
        at(0),
    )
    .unwrap();
    assert!(core.next_value_deadline().is_none());

    core.apply(
        &actor(),
        batch(vec![WriteOperation::SubmitDesired {
            topic: topic("/tracked/desired"),
            value: Value::Integer(1),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(5),
    )
    .unwrap();
    assert_eq!(core.next_value_deadline().unwrap().get(), at(15));
}

#[test]
fn zero_expected_update_interval_is_rejected() {
    let rule = SchemaRule::new(
        Selector::parse("/tracked/**").unwrap(),
        Enforcement::Warn,
        ValueValidator::any(NullPolicy::Allow),
    )
    .unwrap();
    assert!(rule.with_expected_update_interval(seconds(0)).is_err());
}

#[test]
fn diagnostic_slow_consumer_does_not_create_recursive_diagnostics() {
    let mut core = Core::new();
    core.install_schema(freshness_schema(), SchemaInstallMode::RejectInvalid, at(0))
        .unwrap();
    core.apply(
        &actor(),
        batch(vec![
            WriteOperation::PublishState {
                topic: topic("/tracked/one"),
                value: Value::Integer(1),
                expiry: ExpiryUpdate::Clear,
            },
            WriteOperation::PublishState {
                topic: topic("/tracked/two"),
                value: Value::Integer(2),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        at(0),
    )
    .unwrap();
    let mut subscription = core
        .subscribe(
            Selection::new(vec![Selector::parse("/$diagnostics/**").unwrap()]),
            SubscriptionCapacity::new(1).unwrap(),
        )
        .unwrap();
    core.process_deadlines(at(10), &[]).unwrap();

    for (path, value, second) in [("/tracked/one", 1, 11), ("/tracked/two", 2, 12)] {
        core.apply(
            &actor(),
            batch(vec![WriteOperation::PublishState {
                topic: topic(path),
                value: Value::Integer(value),
                expiry: ExpiryUpdate::Clear,
            }]),
            at(second),
        )
        .unwrap();
    }

    assert!(subscription.try_update().is_some());
    assert!(subscription.try_update().is_none());
    assert_eq!(
        subscription.end_reason(),
        Some(SubscriptionEnd::SlowConsumer)
    );
    assert!(
        core.read(&Selection::new(vec![
            Selector::parse("/$diagnostics/**").unwrap()
        ]))
        .nodes()
        .is_empty()
    );
}
