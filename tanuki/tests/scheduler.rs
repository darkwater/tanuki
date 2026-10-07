use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI64, Ordering},
};

use jiff::{SignedDuration, Timestamp as JiffTimestamp};
use tanuki::{
    core::{Core, SchemaInstallMode, SubscriptionCapacity},
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node,
        NonNegativeDuration, Selection, Selector, Timestamp, TopicPath, Value, WriteBatch,
        WriteContext, WriteOperation,
    },
    scheduler::{Clock, DeadlineScheduler},
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueValidator},
};
use tokio::time::{Duration, advance, timeout};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn seconds(value: i64) -> NonNegativeDuration {
    NonNegativeDuration::new(SignedDuration::from_secs(value)).unwrap()
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn test_scheduler() -> (Arc<Mutex<Core>>, DeadlineScheduler, Arc<AtomicI64>) {
    let core = Arc::new(Mutex::new(Core::new()));
    let second = Arc::new(AtomicI64::new(0));
    let clock_second = Arc::clone(&second);
    let clock: Clock = Arc::new(move || at(clock_second.load(Ordering::SeqCst)));
    let scheduler = DeadlineScheduler::start(Arc::clone(&core), clock);
    (core, scheduler, second)
}

#[tokio::test(start_paused = true)]
async fn joined_scheduler_cannot_expire_values_after_server_shutdown() {
    let (core, scheduler, second) = test_scheduler();
    core.lock()
        .unwrap()
        .apply(
            &WriteContext::stateless(ClientName::parse("publisher").unwrap()),
            batch(vec![WriteOperation::PublishState {
                topic: topic("/retained"),
                value: Value::Integer(1),
                expiry: ExpiryUpdate::Set(seconds(10)),
            }]),
            at(0),
        )
        .unwrap();
    scheduler.rescan();
    scheduler.shutdown().await.unwrap();
    second.store(20, Ordering::SeqCst);
    advance(Duration::from_secs(20)).await;
    scheduler.rescan();
    assert!(
        core.lock()
            .unwrap()
            .read(&Selection::new(vec![Selector::parse("/retained").unwrap()]))
            .get(&topic("/retained"))
            .is_some()
    );
}

#[tokio::test(start_paused = true)]
async fn moved_earlier_deadline_wakes_scheduler_and_removes_state() {
    let (core, scheduler, second) = test_scheduler();
    let mut subscription = core
        .lock()
        .unwrap()
        .subscribe(
            Selection::new(vec![Selector::parse("/battery/laptop").unwrap()]),
            SubscriptionCapacity::new(8).unwrap(),
        )
        .unwrap();
    let actor = WriteContext::stateless(ClientName::parse("laptop").unwrap());
    let write = |value, expiry| WriteOperation::PublishState {
        topic: topic("/battery/laptop"),
        value: Value::Integer(value),
        expiry: ExpiryUpdate::Set(seconds(expiry)),
    };
    core.lock()
        .unwrap()
        .apply(&actor, batch(vec![write(1, 100)]), at(0))
        .unwrap();
    scheduler.rescan();
    let _ = subscription.update().await;
    tokio::task::yield_now().await;

    core.lock()
        .unwrap()
        .apply(&actor, batch(vec![write(2, 10)]), at(0))
        .unwrap();
    scheduler.rescan();
    let _ = subscription.update().await;
    tokio::task::yield_now().await;

    second.store(10, Ordering::SeqCst);
    advance(Duration::from_secs(10)).await;
    let expired = timeout(Duration::from_secs(1), subscription.update())
        .await
        .expect("expiry update timed out")
        .expect("subscription closed");
    assert!(matches!(
        expired.changes(),
        [tanuki::core::Change::Removed { topic: removed, .. }]
            if removed == &topic("/battery/laptop")
    ));
}

#[tokio::test(start_paused = true)]
async fn scheduled_grace_release_preserves_submission_and_definition() {
    let (core, scheduler, second) = test_scheduler();
    let mut subscription = core
        .lock()
        .unwrap()
        .subscribe(
            Selection::new(vec![Selector::parse("/lamp/desired").unwrap()]),
            SubscriptionCapacity::new(8).unwrap(),
        )
        .unwrap();
    let session = core
        .lock()
        .unwrap()
        .open_session(ClientName::parse("controller").unwrap(), at(0))
        .unwrap()
        .handle()
        .clone();
    core.lock()
        .unwrap()
        .apply(
            &WriteContext::managed(session.clone()),
            batch(vec![
                WriteOperation::DefineInput {
                    topic: topic("/lamp/desired"),
                    kind: InputKind::Desired,
                    definition: InputDefinition::new(),
                },
                WriteOperation::ClaimInput {
                    topic: topic("/lamp/desired"),
                    release: ClaimRelease::After(seconds(10)),
                },
                WriteOperation::SubmitDesired {
                    topic: topic("/lamp/desired"),
                    value: Value::Integer(70),
                    expiry: ExpiryUpdate::Clear,
                },
            ]),
            at(0),
        )
        .unwrap();
    let _ = subscription.update().await;
    let disconnected = core.lock().unwrap().disconnect(&session, at(0)).unwrap();
    scheduler.schedule_claims(disconnected.pending_releases());

    core.lock()
        .unwrap()
        .apply(
            &WriteContext::stateless(ClientName::parse("automation").unwrap()),
            batch(vec![WriteOperation::SubmitDesired {
                topic: topic("/lamp/desired"),
                value: Value::Integer(80),
                expiry: ExpiryUpdate::Clear,
            }]),
            at(5),
        )
        .unwrap();
    let _ = subscription.update().await;
    tokio::task::yield_now().await;

    second.store(10, Ordering::SeqCst);
    advance(Duration::from_secs(10)).await;
    let released = timeout(Duration::from_secs(1), subscription.update())
        .await
        .expect("claim-release update timed out")
        .expect("subscription closed");
    assert_eq!(released.changes().len(), 1);

    let snapshot = core.lock().unwrap().read(&Selection::new(vec![
        Selector::parse("/lamp/desired").unwrap(),
    ]));
    let Node::Desired(desired) = snapshot.nodes().get(&topic("/lamp/desired")).unwrap() else {
        panic!("input definition must survive grace release");
    };
    assert!(desired.claim().is_none());
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(80));
}

#[tokio::test(start_paused = true)]
async fn expected_update_interval_wakes_scheduler_without_deleting_data() {
    let (core, scheduler, second) = test_scheduler();
    core.lock()
        .unwrap()
        .install_schema(
            Schema::new(
                SchemaName::parse("freshness").unwrap(),
                vec![
                    SchemaRule::new(
                        Selector::parse("/sensor/*").unwrap(),
                        Enforcement::Warn,
                        ValueValidator::any(NullPolicy::Allow),
                    )
                    .unwrap()
                    .with_expected_update_interval(seconds(10))
                    .unwrap(),
                ],
            )
            .unwrap(),
            SchemaInstallMode::RejectInvalid,
            at(0),
        )
        .unwrap();
    let mut subscription = core
        .lock()
        .unwrap()
        .subscribe(
            Selection::new(vec![Selector::parse("/$diagnostics/**").unwrap()]),
            SubscriptionCapacity::new(8).unwrap(),
        )
        .unwrap();
    core.lock()
        .unwrap()
        .apply(
            &WriteContext::stateless(ClientName::parse("sensor").unwrap()),
            batch(vec![WriteOperation::PublishState {
                topic: topic("/sensor/temperature"),
                value: Value::Integer(20),
                expiry: ExpiryUpdate::Clear,
            }]),
            at(0),
        )
        .unwrap();
    scheduler.rescan();
    tokio::task::yield_now().await;

    second.store(10, Ordering::SeqCst);
    advance(Duration::from_secs(10)).await;
    let overdue = timeout(Duration::from_secs(1), subscription.update())
        .await
        .expect("freshness update timed out")
        .expect("subscription closed");
    assert!(matches!(
        overdue.changes(),
        [tanuki::core::Change::Upsert { topic: condition, .. }]
            if condition == &topic("/$diagnostics/freshness/sensor/temperature")
    ));
    assert!(
        core.lock()
            .unwrap()
            .read(&Selection::new(vec![
                Selector::parse("/sensor/temperature").unwrap()
            ]))
            .nodes()
            .contains_key(&topic("/sensor/temperature"))
    );
}
