use jiff::{SignedDuration, Timestamp as JiffTimestamp};
use tanuki::{
    core::{Change, Core, CoreError, Diagnostic},
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node,
        NonNegativeDuration, Selection, Selector, Timestamp, TopicPath, Value, WriteBatch,
        WriteContext, WriteOperation,
    },
};

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn name(value: &str) -> ClientName {
    ClientName::parse(value).unwrap()
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn all() -> Selection {
    Selection::new(vec![Selector::parse("/**").unwrap()])
}

fn define(path: &str, kind: InputKind) -> WriteOperation {
    WriteOperation::DefineInput {
        topic: topic(path),
        kind,
        definition: InputDefinition::new(),
    }
}

#[test]
fn stateless_definition_and_submission_create_unclaimed_pending_intent() {
    let mut core = Core::new();
    let stateless = WriteContext::stateless(name("phone automation"));
    core.apply(
        &stateless,
        batch(vec![
            define("/heating/desired", InputKind::Desired),
            WriteOperation::SubmitDesired {
                topic: topic("/heating/desired"),
                value: Value::Integer(21),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        at(10),
    )
    .unwrap();

    let snapshot = core.read(&all());
    let Node::Desired(desired) = snapshot.get(&topic("/heating/desired")).unwrap() else {
        panic!("expected desired input")
    };
    assert!(desired.claim().is_none());
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(21));
    assert_eq!(
        desired.current().unwrap().last_write().client().as_str(),
        "phone automation"
    );
    assert_eq!(core.managed_session_count(), 0);
}

#[test]
fn claiming_preserves_value_and_submission_preserves_claim() {
    let mut core = Core::new();
    let phone = WriteContext::stateless(name("phone automation"));
    core.apply(
        &phone,
        batch(vec![
            define("/lamp/desired-brightness", InputKind::Desired),
            WriteOperation::SubmitDesired {
                topic: topic("/lamp/desired-brightness"),
                value: Value::Integer(70),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        at(10),
    )
    .unwrap();

    let opened = core.open_session(name("lamp controller"), at(20)).unwrap();
    let controller = WriteContext::managed(opened.handle().clone());
    core.apply(
        &controller,
        batch(vec![WriteOperation::ClaimInput {
            topic: topic("/lamp/desired-brightness"),
            release: ClaimRelease::Immediate,
        }]),
        at(20),
    )
    .unwrap();
    core.apply(
        &phone,
        batch(vec![WriteOperation::SubmitDesired {
            topic: topic("/lamp/desired-brightness"),
            value: Value::Integer(75),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(30),
    )
    .unwrap();

    let snapshot = core.read(&all());
    let Node::Desired(desired) = snapshot.get(&topic("/lamp/desired-brightness")).unwrap() else {
        panic!("expected desired input")
    };
    assert_eq!(desired.claim().unwrap().owner().as_str(), "lamp controller");
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(75));
    assert_eq!(
        desired.current().unwrap().last_write().client().as_str(),
        "phone automation"
    );
}

#[test]
fn duplicate_managed_name_invalidates_old_handle_and_old_cleanup_is_harmless() {
    let mut core = Core::new();
    let first = core
        .open_session(name("laptop battery"), at(10))
        .unwrap()
        .handle()
        .clone();
    let replacement = core.open_session(name("laptop battery"), at(20)).unwrap();
    assert!(matches!(
        replacement.warnings(),
        [Diagnostic::SessionReplaced { .. }]
    ));

    let old_write = core.apply(
        &WriteContext::managed(first.clone()),
        batch(vec![WriteOperation::PublishState {
            topic: topic("/battery/laptop"),
            value: Value::Integer(50),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(30),
    );
    assert!(matches!(old_write, Err(CoreError::SessionExpired { .. })));
    assert!(!core.disconnect(&first, at(30)).unwrap().was_current());

    core.apply(
        &WriteContext::managed(replacement.handle().clone()),
        batch(vec![WriteOperation::PublishState {
            topic: topic("/battery/laptop"),
            value: Value::Integer(80),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(30),
    )
    .unwrap();
    assert_eq!(core.managed_session_count(), 1);
}

#[test]
fn same_name_stateless_actor_cannot_claim_or_displace_managed_session() {
    let mut core = Core::new();
    let managed = core.open_session(name("controller"), at(10)).unwrap();
    core.apply(
        &WriteContext::stateless(name("author")),
        batch(vec![define("/lamp/desired", InputKind::Desired)]),
        at(10),
    )
    .unwrap();

    let result = core.apply(
        &WriteContext::stateless(name("controller")),
        batch(vec![WriteOperation::ClaimInput {
            topic: topic("/lamp/desired"),
            release: ClaimRelease::Immediate,
        }]),
        at(20),
    );
    assert!(matches!(result, Err(CoreError::SessionRequired { .. })));
    assert_eq!(core.managed_session_count(), 1);
    assert_eq!(managed.handle().client().as_str(), "controller");
}

#[test]
fn claim_collision_warns_and_replaces_owner() {
    let mut core = Core::new();
    core.apply(
        &WriteContext::stateless(name("author")),
        batch(vec![define("/lamp/desired", InputKind::Desired)]),
        at(10),
    )
    .unwrap();
    let first = core.open_session(name("first controller"), at(10)).unwrap();
    let second = core
        .open_session(name("second controller"), at(10))
        .unwrap();
    core.apply(
        &WriteContext::managed(first.handle().clone()),
        batch(vec![WriteOperation::ClaimInput {
            topic: topic("/lamp/desired"),
            release: ClaimRelease::Immediate,
        }]),
        at(20),
    )
    .unwrap();
    let outcome = core
        .apply(
            &WriteContext::managed(second.handle().clone()),
            batch(vec![WriteOperation::ClaimInput {
                topic: topic("/lamp/desired"),
                release: ClaimRelease::Immediate,
            }]),
            at(30),
        )
        .unwrap();

    assert!(matches!(
        outcome.warnings(),
        [Diagnostic::InputClaimReplaced { previous, replacement, .. }]
            if previous.as_str() == "first controller"
                && replacement.as_str() == "second controller"
    ));
}

#[test]
fn ownerless_command_is_delivered_once_and_never_snapshotted_as_payload() {
    let mut core = Core::new();
    let outcome = core
        .apply(
            &WriteContext::stateless(name("remote")),
            batch(vec![
                define("/lamp/toggle", InputKind::Command),
                WriteOperation::SubmitCommand {
                    topic: topic("/lamp/toggle"),
                    value: Value::String("toggle".to_owned()),
                },
            ]),
            at(10),
        )
        .unwrap();

    assert!(outcome.update().changes().iter().any(|change| matches!(
        change,
        Change::Command { command, .. }
            if command.value() == &Value::String("toggle".to_owned())
    )));
    let snapshot = core.read(&all());
    let node = snapshot.get(&topic("/lamp/toggle")).unwrap();
    assert!(matches!(node, Node::Command(_)));
    assert_eq!(node.retained_value(), None);
}

#[test]
fn disconnect_immediately_releases_claim_but_preserves_desired_value() {
    let mut core = Core::new();
    let opened = core.open_session(name("controller"), at(10)).unwrap();
    let actor = WriteContext::managed(opened.handle().clone());
    core.apply(
        &actor,
        batch(vec![
            define("/lamp/desired", InputKind::Desired),
            WriteOperation::ClaimInput {
                topic: topic("/lamp/desired"),
                release: ClaimRelease::Immediate,
            },
            WriteOperation::SubmitDesired {
                topic: topic("/lamp/desired"),
                value: Value::Integer(70),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        at(10),
    )
    .unwrap();

    let disconnected = core.disconnect(opened.handle(), at(20)).unwrap();
    assert!(disconnected.was_current());
    assert!(disconnected.update().is_some());
    let snapshot = core.read(&all());
    let Node::Desired(desired) = snapshot.get(&topic("/lamp/desired")).unwrap() else {
        panic!("expected desired input")
    };
    assert!(desired.claim().is_none());
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(70));
}

#[test]
fn disconnect_with_grace_returns_guarded_release_work() {
    let mut core = Core::new();
    let opened = core.open_session(name("controller"), at(10)).unwrap();
    let grace = NonNegativeDuration::new(SignedDuration::from_secs(30)).unwrap();
    core.apply(
        &WriteContext::managed(opened.handle().clone()),
        batch(vec![
            define("/lamp/toggle", InputKind::Command),
            WriteOperation::ClaimInput {
                topic: topic("/lamp/toggle"),
                release: ClaimRelease::After(grace),
            },
        ]),
        at(10),
    )
    .unwrap();

    let disconnected = core.disconnect(opened.handle(), at(20)).unwrap();
    assert_eq!(disconnected.pending_releases().len(), 1);
    assert_eq!(disconnected.pending_releases()[0].deadline().get(), at(50));
    let snapshot = core.read(&all());
    let Node::Command(command) = snapshot.get(&topic("/lamp/toggle")).unwrap() else {
        panic!("expected command input")
    };
    assert_eq!(
        command.claim().unwrap().id(),
        disconnected.pending_releases()[0].claim()
    );
}

#[test]
fn replacement_releases_immediate_claim_and_stale_disconnect_cannot_touch_reclaim() {
    let mut core = Core::new();
    let first = core.open_session(name("controller"), at(10)).unwrap();
    core.apply(
        &WriteContext::managed(first.handle().clone()),
        batch(vec![
            define("/lamp/desired", InputKind::Desired),
            WriteOperation::ClaimInput {
                topic: topic("/lamp/desired"),
                release: ClaimRelease::Immediate,
            },
        ]),
        at(10),
    )
    .unwrap();

    let replacement = core.open_session(name("controller"), at(20)).unwrap();
    assert!(replacement.update().is_some());
    let after_replacement = core.read(&all());
    let Node::Desired(desired) = after_replacement.get(&topic("/lamp/desired")).unwrap() else {
        panic!("expected desired input")
    };
    assert!(desired.claim().is_none());

    core.apply(
        &WriteContext::managed(replacement.handle().clone()),
        batch(vec![WriteOperation::ClaimInput {
            topic: topic("/lamp/desired"),
            release: ClaimRelease::Immediate,
        }]),
        at(20),
    )
    .unwrap();
    assert!(
        !core
            .disconnect(first.handle(), at(30))
            .unwrap()
            .was_current()
    );

    let final_snapshot = core.read(&all());
    let Node::Desired(desired) = final_snapshot.get(&topic("/lamp/desired")).unwrap() else {
        panic!("expected desired input")
    };
    assert_eq!(
        desired.claim().unwrap().session(),
        replacement.handle().id()
    );
}

#[test]
fn claimed_definition_requires_session_authority_and_failure_rolls_back_batch() {
    let mut core = Core::new();
    let controller = core.open_session(name("controller"), at(10)).unwrap();
    core.apply(
        &WriteContext::managed(controller.handle().clone()),
        batch(vec![
            define("/lamp/desired", InputKind::Desired),
            WriteOperation::ClaimInput {
                topic: topic("/lamp/desired"),
                release: ClaimRelease::Immediate,
            },
        ]),
        at(10),
    )
    .unwrap();
    let before = core.read(&all());

    let result = core.apply(
        &WriteContext::stateless(name("controller")),
        batch(vec![
            WriteOperation::PublishState {
                topic: topic("/unrelated"),
                value: Value::Bool(true),
                expiry: ExpiryUpdate::Clear,
            },
            define("/lamp/desired", InputKind::Desired),
        ]),
        at(20),
    );

    assert!(matches!(
        result,
        Err(CoreError::ClaimAuthorityRequired { .. })
    ));
    assert_eq!(core.read(&all()), before);
}

#[test]
fn same_name_stateless_output_does_not_displace_managed_authority() {
    let mut core = Core::new();
    let opened = core.open_session(name("battery agent"), at(10)).unwrap();
    core.apply(
        &WriteContext::stateless(name("battery agent")),
        batch(vec![WriteOperation::PublishState {
            topic: topic("/battery/phone"),
            value: Value::Integer(90),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(20),
    )
    .unwrap();
    core.apply(
        &WriteContext::managed(opened.handle().clone()),
        batch(vec![WriteOperation::PublishState {
            topic: topic("/battery/laptop"),
            value: Value::Integer(80),
            expiry: ExpiryUpdate::Clear,
        }]),
        at(30),
    )
    .unwrap();

    assert_eq!(core.managed_session_count(), 1);
}
