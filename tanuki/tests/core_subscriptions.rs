use jiff::Timestamp as JiffTimestamp;
use std::sync::{Arc, Barrier, Mutex, mpsc};
use tanuki::{
    core::{Change, Core, SubscriptionCapacity, SubscriptionEnd},
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node, Selection,
        Selector, Timestamp, TopicPath, Value, WriteBatch, WriteContext, WriteOperation,
    },
};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn selection(values: &[&str]) -> Selection {
    Selection::new(
        values
            .iter()
            .map(|value| Selector::parse(value).unwrap())
            .collect(),
    )
}

fn actor() -> WriteContext {
    WriteContext::stateless(ClientName::parse("test producer").unwrap())
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn state(path: &str, value: i64) -> WriteOperation {
    WriteOperation::PublishState {
        topic: topic(path),
        value: Value::Integer(value),
        expiry: ExpiryUpdate::Clear,
    }
}

fn capacity(value: usize) -> SubscriptionCapacity {
    SubscriptionCapacity::new(value).unwrap()
}

#[test]
fn snapshot_then_newer_update_has_no_registration_gap() {
    let mut core = Core::new();
    core.apply(&actor(), batch(vec![state("/battery/phone", 70)]), at(1))
        .unwrap();

    let mut subscription = core
        .subscribe(selection(&["/battery/*"]), capacity(4))
        .unwrap();
    assert_eq!(subscription.snapshot().sequence().get(), 1);
    assert!(
        subscription
            .snapshot()
            .get(&topic("/battery/phone"))
            .is_some()
    );

    core.apply(&actor(), batch(vec![state("/battery/phone", 71)]), at(2))
        .unwrap();
    let update = subscription.try_update().expect("newer update is queued");
    assert_eq!(update.sequence().get(), 2);
    assert!(matches!(update.changes(), [Change::Upsert { .. }]));
}

#[test]
fn overlapping_selectors_do_not_duplicate_changes_and_batches_stay_atomic() {
    let mut core = Core::new();
    let mut subscription = core
        .subscribe(selection(&["/lamp/**", "/lamp/*"]), capacity(4))
        .unwrap();
    assert!(subscription.snapshot().nodes().is_empty());

    core.apply(
        &actor(),
        batch(vec![state("/lamp/hue", 120), state("/lamp/brightness", 70)]),
        at(1),
    )
    .unwrap();
    let update = subscription.try_update().unwrap();
    assert_eq!(update.changes().len(), 2);
    assert!(subscription.try_update().is_none());
}

#[test]
fn instant_occurrence_is_live_only_and_empty_selection_stays_empty() {
    let mut events = Core::new();
    let mut subscription = events
        .subscribe(selection(&["/doorbell/**"]), capacity(4))
        .unwrap();
    let mut empty = events
        .subscribe(Selection::new(Vec::new()), capacity(4))
        .unwrap();
    assert!(empty.snapshot().nodes().is_empty());

    events
        .apply(
            &actor(),
            batch(vec![WriteOperation::PublishEvent {
                topic: topic("/doorbell/rang"),
                value: Value::Bool(true),
            }]),
            at(1),
        )
        .unwrap();
    let update = subscription.try_update().unwrap();
    assert!(
        update
            .changes()
            .iter()
            .any(|change| matches!(change, Change::Occurrence { .. }))
    );
    assert!(empty.try_update().is_none());

    let snapshot = events
        .subscribe(selection(&["/doorbell/**"]), capacity(4))
        .unwrap();
    let event = snapshot.snapshot().get(&topic("/doorbell/rang")).unwrap();
    let Node::Event(_) = event else {
        panic!("expected event metadata")
    };
    assert_eq!(event.retained_value(), None);
}

#[test]
fn desired_payload_clear_is_an_explicit_upsert() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![
            WriteOperation::DefineInput {
                topic: topic("/lamp/desired"),
                kind: InputKind::Desired,
                definition: InputDefinition::new(),
            },
            WriteOperation::SubmitDesired {
                topic: topic("/lamp/desired"),
                value: Value::Integer(70),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        at(1),
    )
    .unwrap();
    let mut subscription = core
        .subscribe(selection(&["/lamp/desired"]), capacity(4))
        .unwrap();

    core.apply(
        &actor(),
        batch(vec![WriteOperation::ClearDesired {
            topic: topic("/lamp/desired"),
        }]),
        at(2),
    )
    .unwrap();
    let update = subscription.try_update().unwrap();
    let [
        Change::Upsert {
            node: Node::Desired(desired),
            ..
        },
    ] = update.changes()
    else {
        panic!("expected one desired upsert")
    };
    assert!(desired.current().is_none());
}

#[test]
fn claim_only_metadata_change_and_removal_are_explicit() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![WriteOperation::DefineInput {
            topic: topic("/lamp/desired"),
            kind: InputKind::Desired,
            definition: InputDefinition::new(),
        }]),
        at(1),
    )
    .unwrap();
    let opened = core
        .open_session(ClientName::parse("controller").unwrap(), at(1))
        .unwrap();
    let mut subscription = core
        .subscribe(selection(&["/lamp/**"]), capacity(4))
        .unwrap();

    core.apply(
        &WriteContext::managed(opened.handle().clone()),
        batch(vec![WriteOperation::ClaimInput {
            topic: topic("/lamp/desired"),
            release: ClaimRelease::Immediate,
        }]),
        at(2),
    )
    .unwrap();
    let claim_update = subscription.try_update().unwrap();
    let [
        Change::Upsert {
            node: Node::Desired(desired),
            ..
        },
    ] = claim_update.changes()
    else {
        panic!("expected one desired metadata upsert")
    };
    assert!(desired.claim().is_some());
    assert!(desired.current().is_none());

    core.apply(
        &actor(),
        batch(vec![WriteOperation::RemoveNode {
            topic: topic("/lamp/desired"),
        }]),
        at(3),
    )
    .unwrap();
    assert!(matches!(
        subscription.try_update().unwrap().changes(),
        [Change::Removed {
            previous: Node::Desired(_),
            ..
        }]
    ));
}

#[test]
fn full_slow_queue_disconnects_without_blocking_other_subscribers() {
    let mut core = Core::new();
    let mut slow = core
        .subscribe(selection(&["/battery/**"]), capacity(1))
        .unwrap();
    let mut fast = core
        .subscribe(selection(&["/battery/**"]), capacity(4))
        .unwrap();

    core.apply(&actor(), batch(vec![state("/battery/phone", 70)]), at(1))
        .unwrap();
    assert!(fast.try_update().is_some());
    core.apply(&actor(), batch(vec![state("/battery/phone", 71)]), at(2))
        .unwrap();

    assert_eq!(slow.end_reason(), Some(SubscriptionEnd::SlowConsumer));
    assert!(fast.try_update().is_some());
    assert!(
        slow.try_update().is_some(),
        "already queued batch remains readable"
    );
}

#[test]
fn zero_capacity_is_rejected_by_construction() {
    assert!(SubscriptionCapacity::new(0).is_err());
}

#[test]
fn concurrent_registration_linearizes_into_snapshot_or_following_update() {
    for _ in 0..32 {
        let core = Arc::new(Mutex::new(Core::new()));
        let barrier = Arc::new(Barrier::new(3));
        let (subscription_tx, subscription_rx) = mpsc::channel();

        let subscriber_core = Arc::clone(&core);
        let subscriber_barrier = Arc::clone(&barrier);
        let subscriber = std::thread::spawn(move || {
            subscriber_barrier.wait();
            let subscription = subscriber_core
                .lock()
                .unwrap()
                .subscribe(selection(&["/race"]), capacity(1))
                .unwrap();
            subscription_tx.send(subscription).unwrap();
        });

        let writer_core = Arc::clone(&core);
        let writer_barrier = Arc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            writer_core
                .lock()
                .unwrap()
                .apply(&actor(), batch(vec![state("/race", 1)]), at(1))
                .unwrap();
        });

        barrier.wait();
        subscriber.join().unwrap();
        writer.join().unwrap();
        let mut subscription = subscription_rx.recv().unwrap();
        let in_snapshot = subscription.snapshot().get(&topic("/race")).is_some();
        let in_update = subscription.try_update().is_some();
        assert_ne!(in_snapshot, in_update, "write must appear exactly once");
    }
}
