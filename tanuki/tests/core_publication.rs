use jiff::{SignedDuration, Timestamp as JiffTimestamp};
use tanuki::{
    core::{Change, Core, CoreError, Diagnostic},
    domain::{
        ClientName, ExpiryUpdate, Node, NodeKind, NonNegativeDuration, Selection, Selector,
        Timestamp, TopicPath, Value, WriteBatch, WriteContext, WriteOperation,
    },
};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn actor(name: &str) -> WriteContext {
    WriteContext::stateless(ClientName::parse(name).unwrap())
}

fn topic(path: &str) -> TopicPath {
    TopicPath::parse(path).unwrap()
}

fn state(path: &str, value: i64) -> WriteOperation {
    WriteOperation::PublishState {
        topic: topic(path),
        value: Value::Integer(value),
        expiry: ExpiryUpdate::Preserve,
    }
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn all() -> Selection {
    Selection::new(vec![Selector::parse("/**").unwrap()])
}

#[test]
fn mixed_batch_rolls_back_when_a_system_write_is_rejected() {
    let mut core = Core::new();
    let result = core.apply(
        &actor("lamp controller"),
        batch(vec![state("/lamp/hue", 120), state("/$system/secret", 1)]),
        at(10),
    );

    assert!(matches!(result, Err(CoreError::SystemTopic { .. })));
    assert!(core.read(&all()).nodes().is_empty());
}

#[test]
fn staged_state_and_event_are_discarded_on_late_failure() {
    let mut core = Core::new();
    let one_second = NonNegativeDuration::new(SignedDuration::from_secs(1)).unwrap();
    let result = core.apply(
        &actor("mixed publisher"),
        batch(vec![
            WriteOperation::PublishEvent {
                topic: topic("/doorbell/rang"),
                value: Value::String("ding".to_owned()),
            },
            state("/lamp/hue", 120),
            WriteOperation::PublishState {
                topic: topic("/clock/overflow"),
                value: Value::Integer(1),
                expiry: ExpiryUpdate::Set(one_second),
            },
        ]),
        Timestamp::new(JiffTimestamp::MAX),
    );

    assert!(matches!(result, Err(CoreError::DeadlineOutOfRange { .. })));
    let snapshot = core.read(&all());
    assert_eq!(snapshot.sequence().get(), 0);
    assert!(snapshot.nodes().is_empty());
}

#[test]
fn two_state_writes_form_one_coherent_commit_batch() {
    let mut core = Core::new();
    let outcome = core
        .apply(
            &actor("lamp controller"),
            batch(vec![state("/lamp/hue", 120), state("/lamp/brightness", 70)]),
            at(10),
        )
        .unwrap();

    assert_eq!(outcome.update().sequence().get(), 1);
    assert_eq!(outcome.update().changes().len(), 2);
    assert!(
        outcome
            .update()
            .changes()
            .iter()
            .all(|change| matches!(change, Change::Upsert { .. }))
    );
    let snapshot = core.read(&all());
    assert_eq!(snapshot.sequence().get(), 1);
    assert_eq!(snapshot.nodes().len(), 2);
}

#[test]
fn duplicate_state_write_refreshes_provenance_and_expiry() {
    let mut core = Core::new();
    let expiry = NonNegativeDuration::new(SignedDuration::from_secs(60)).unwrap();
    let write = |expiry| WriteOperation::PublishState {
        topic: topic("/battery/laptop"),
        value: Value::Integer(82),
        expiry,
    };

    core.apply(
        &actor("battery script"),
        batch(vec![write(ExpiryUpdate::Set(expiry))]),
        at(10),
    )
    .unwrap();
    core.apply(
        &actor("battery script"),
        batch(vec![write(ExpiryUpdate::Set(expiry))]),
        at(20),
    )
    .unwrap();

    let snapshot = core.read(&all());
    let retained = snapshot
        .get(&topic("/battery/laptop"))
        .unwrap()
        .retained_value()
        .unwrap();
    assert_eq!(retained.last_write().at(), at(20));
    assert_eq!(retained.expires_at().unwrap().get(), at(80));
}

#[test]
fn output_takeover_warns_and_changes_attribution() {
    let mut core = Core::new();
    core.apply(
        &actor("first publisher"),
        batch(vec![state("/tv/power", 0)]),
        at(10),
    )
    .unwrap();
    let outcome = core
        .apply(
            &actor("replacement publisher"),
            batch(vec![state("/tv/power", 1)]),
            at(20),
        )
        .unwrap();

    assert!(matches!(
        outcome.warnings(),
        [Diagnostic::OutputOwnerChanged { previous, replacement, .. }]
            if previous.as_str() == "first publisher"
                && replacement.as_str() == "replacement publisher"
    ));
    let snapshot = core.read(&all());
    let retained = snapshot
        .get(&topic("/tv/power"))
        .unwrap()
        .retained_value()
        .unwrap();
    assert_eq!(
        retained.last_write().client().as_str(),
        "replacement publisher"
    );
}

#[test]
fn event_occurrence_is_committed_but_not_retained_in_snapshot() {
    let mut core = Core::new();
    let outcome = core
        .apply(
            &actor("doorbell"),
            batch(vec![WriteOperation::PublishEvent {
                topic: topic("/doorbell/rang"),
                value: Value::String("ding".to_owned()),
            }]),
            at(10),
        )
        .unwrap();

    assert!(matches!(
        outcome.update().changes(),
        [Change::Upsert { .. }, Change::Occurrence { event, .. }]
            if event.value() == &Value::String("ding".to_owned())
    ));
    let snapshot = core.read(&all());
    let node = snapshot.get(&topic("/doorbell/rang")).unwrap();
    assert_eq!(node.kind(), NodeKind::Event);
    assert_eq!(node.retained_value(), None);
}

#[test]
fn freeform_kind_change_warns_and_replaces_incompatible_state() {
    let mut core = Core::new();
    core.apply(
        &actor("doorbell"),
        batch(vec![state("/doorbell/rang", 0)]),
        at(10),
    )
    .unwrap();
    let outcome = core
        .apply(
            &actor("doorbell"),
            batch(vec![WriteOperation::PublishEvent {
                topic: topic("/doorbell/rang"),
                value: Value::String("ding".to_owned()),
            }]),
            at(20),
        )
        .unwrap();

    assert!(outcome.warnings().iter().any(|warning| matches!(
        warning,
        Diagnostic::NodeKindChanged {
            previous: NodeKind::State,
            replacement: NodeKind::Event,
            ..
        }
    )));
    assert!(matches!(
        core.read(&all()).get(&topic("/doorbell/rang")),
        Some(Node::Event(_))
    ));
}

#[test]
fn repeated_retained_operations_run_in_order_and_publish_only_the_final_shape() {
    let mut core = Core::new();
    let outcome = core
        .apply(
            &actor("lamp controller"),
            batch(vec![state("/lamp/hue", 120), state("/lamp/hue", 121)]),
            at(10),
        )
        .unwrap();

    assert!(matches!(
        outcome.update().changes(),
        [Change::Upsert { node: Node::State(node), .. }]
            if node.current().value() == &Value::Integer(121)
    ));
}

#[test]
fn create_then_remove_in_one_batch_has_no_visible_retained_change() {
    let mut core = Core::new();
    let outcome = core
        .apply(
            &actor("lamp controller"),
            batch(vec![
                state("/lamp/hue", 120),
                WriteOperation::RemoveNode {
                    topic: topic("/lamp/hue"),
                },
            ]),
            at(10),
        )
        .unwrap();

    assert!(outcome.update().changes().is_empty());
    assert!(core.read(&all()).nodes().is_empty());
}

#[test]
fn repeated_instant_operations_preserve_occurrence_order_with_one_final_node() {
    let mut core = Core::new();
    let event = |value: &str| WriteOperation::PublishEvent {
        topic: topic("/doorbell/rang"),
        value: Value::String(value.to_owned()),
    };
    let outcome = core
        .apply(
            &actor("doorbell"),
            batch(vec![event("ding"), event("dong")]),
            at(10),
        )
        .unwrap();

    assert!(matches!(
        outcome.update().changes(),
        [
            Change::Upsert { node: Node::Event(_), .. },
            Change::Occurrence { event: first, .. },
            Change::Occurrence { event: second, .. },
        ] if first.value() == &Value::String("ding".to_owned())
            && second.value() == &Value::String("dong".to_owned())
    ));
}

#[test]
fn snapshot_selection_filters_topics_and_removal_carries_previous_state() {
    let mut core = Core::new();
    core.apply(
        &actor("room publisher"),
        batch(vec![state("/battery/laptop", 82), state("/tv/power", 1)]),
        at(10),
    )
    .unwrap();

    let battery = Selection::new(vec![Selector::parse("/battery/*").unwrap()]);
    let snapshot = core.read(&battery);
    assert_eq!(snapshot.nodes().len(), 1);
    assert!(snapshot.get(&topic("/battery/laptop")).is_some());

    let outcome = core
        .apply(
            &actor("room publisher"),
            batch(vec![WriteOperation::RemoveNode {
                topic: topic("/battery/laptop"),
            }]),
            at(20),
        )
        .unwrap();
    assert!(matches!(
        outcome.update().changes(),
        [Change::Removed { topic: removed, previous: Node::State(_) }]
            if removed == &topic("/battery/laptop")
    ));
    assert!(core.read(&battery).nodes().is_empty());
}
