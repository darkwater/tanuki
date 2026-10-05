use jiff::{SignedDuration, Timestamp as JiffTimestamp};
use tanuki::{
    core::Core,
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node,
        NonNegativeDuration, Selection, Selector, Timestamp, TopicPath, Value, WriteBatch,
        WriteContext, WriteOperation,
    },
};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn seconds(value: i64) -> NonNegativeDuration {
    NonNegativeDuration::new(SignedDuration::from_secs(value)).unwrap()
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn actor(name: &str) -> WriteContext {
    WriteContext::stateless(ClientName::parse(name).unwrap())
}

#[test]
fn equal_deadlines_remove_state_and_clear_only_desired_payload_in_one_commit() {
    let mut core = Core::new();
    let controller = core
        .open_session(ClientName::parse("controller").unwrap(), at(0))
        .unwrap()
        .handle()
        .clone();
    core.apply(
        &WriteContext::managed(controller),
        batch(vec![
            WriteOperation::PublishState {
                topic: topic("/battery/phone"),
                value: Value::Integer(50),
                expiry: ExpiryUpdate::Set(seconds(10)),
            },
            WriteOperation::DefineInput {
                topic: topic("/lamp/desired"),
                kind: InputKind::Desired,
                definition: InputDefinition::new(),
            },
            WriteOperation::ClaimInput {
                topic: topic("/lamp/desired"),
                release: ClaimRelease::Immediate,
            },
            WriteOperation::SubmitDesired {
                topic: topic("/lamp/desired"),
                value: Value::Integer(70),
                expiry: ExpiryUpdate::Set(seconds(10)),
            },
        ]),
        at(0),
    )
    .unwrap();

    let update = core.process_deadlines(at(10), &[]).unwrap().unwrap();
    assert_eq!(update.changes().len(), 2);
    let snapshot = core.read(&Selection::new(vec![Selector::parse("/**").unwrap()]));
    assert!(!snapshot.nodes().contains_key(&topic("/battery/phone")));
    let Node::Desired(desired) = snapshot.nodes().get(&topic("/lamp/desired")).unwrap() else {
        panic!("desired definition must survive payload expiry");
    };
    assert!(desired.current().is_none());
    assert!(desired.claim().is_some());
}

#[test]
fn refreshing_a_value_guards_it_from_its_old_deadline() {
    let mut core = Core::new();
    let write = |value, expiry| WriteOperation::PublishState {
        topic: topic("/battery/laptop"),
        value: Value::Integer(value),
        expiry: ExpiryUpdate::Set(seconds(expiry)),
    };
    core.apply(&actor("laptop"), batch(vec![write(10, 10)]), at(0))
        .unwrap();
    core.apply(&actor("laptop"), batch(vec![write(11, 11)]), at(9))
        .unwrap();

    assert_eq!(core.next_value_deadline().unwrap().get(), at(20));
    assert!(core.process_deadlines(at(10), &[]).unwrap().is_none());
    assert!(
        core.read(&Selection::new(vec![
            Selector::parse("/battery/laptop").unwrap()
        ]))
        .nodes()
        .contains_key(&topic("/battery/laptop"))
    );
    assert!(core.process_deadlines(at(20), &[]).unwrap().is_some());
}

#[test]
fn reclaim_before_old_grace_deadline_preserves_new_submission_and_claim() {
    let mut core = Core::new();
    let first = core
        .open_session(ClientName::parse("controller").unwrap(), at(0))
        .unwrap()
        .handle()
        .clone();
    core.apply(
        &WriteContext::managed(first.clone()),
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
    let disconnected = core.disconnect(&first, at(1)).unwrap();
    let old_release = disconnected.pending_releases().to_vec();

    core.apply(
        &actor("remote"),
        batch(vec![WriteOperation::SubmitDesired {
            topic: topic("/lamp/desired"),
            value: Value::Integer(80),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(2),
    )
    .unwrap();
    let replacement = core
        .open_session(ClientName::parse("controller").unwrap(), at(5))
        .unwrap()
        .handle()
        .clone();
    core.apply(
        &WriteContext::managed(replacement),
        batch(vec![WriteOperation::ClaimInput {
            topic: topic("/lamp/desired"),
            release: ClaimRelease::After(seconds(10)),
        }]),
        at(5),
    )
    .unwrap();
    assert!(
        core.process_deadlines(at(11), &old_release)
            .unwrap()
            .is_none()
    );
    let snapshot = core.read(&Selection::new(vec![
        Selector::parse("/lamp/desired").unwrap(),
    ]));
    let Node::Desired(desired) = snapshot.nodes().get(&topic("/lamp/desired")).unwrap() else {
        panic!("lamp input must remain desired");
    };
    assert!(desired.claim().is_some());
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(80));
}
