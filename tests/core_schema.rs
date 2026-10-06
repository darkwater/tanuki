use tanuki::{
    core::{Core, CoreError, Diagnostic, SchemaInstallMode},
    domain::{
        ClientName, ExpiryUpdate, InputDefinition, InputKind, Node, NodeKind, Selection, Selector,
        Timestamp, TopicPath, Value, ValueKind, WriteBatch, WriteContext, WriteOperation,
    },
    schema::{
        Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueCast, ValueValidator,
        ViolationKind,
    },
};

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn now() -> Timestamp {
    Timestamp::new("2026-10-06T00:00:00Z".parse().unwrap())
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn actor() -> WriteContext {
    WriteContext::stateless(ClientName::parse("test").unwrap())
}

fn all() -> Selection {
    Selection::new(vec![Selector::parse("/**").unwrap()])
}

fn integer_schema(enforcement: Enforcement) -> Schema {
    Schema::new(
        SchemaName::parse("battery").unwrap(),
        vec![
            SchemaRule::new(
                Selector::parse("/battery/*").unwrap(),
                enforcement,
                ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap(),
            )
            .unwrap()
            .with_cast(ValueCast::StringToInteger)
            .unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn writes_are_cast_and_validated_before_the_atomic_commit() {
    let mut core = Core::new();
    core.install_schema(
        integer_schema(Enforcement::Deny),
        SchemaInstallMode::RejectInvalid,
    )
    .unwrap();

    let outcome = core
        .apply(
            &actor(),
            batch(vec![WriteOperation::PublishState {
                topic: topic("/battery/phone"),
                value: Value::String("42".to_owned()),
                expiry: ExpiryUpdate::Clear,
            }]),
            now(),
        )
        .unwrap();
    let Node::State(state) = core
        .read(&all())
        .get(&topic("/battery/phone"))
        .unwrap()
        .clone()
    else {
        panic!("expected state node");
    };
    assert_eq!(state.current().value(), &Value::Integer(42));
    assert!(outcome.warnings().is_empty());

    let sequence = outcome.update().sequence();
    let denied = core.apply(
        &actor(),
        batch(vec![
            WriteOperation::PublishState {
                topic: topic("/unrelated"),
                value: Value::Bool(true),
                expiry: ExpiryUpdate::Clear,
            },
            WriteOperation::PublishState {
                topic: topic("/battery/phone"),
                value: Value::String("101".to_owned()),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        now(),
    );
    assert!(matches!(
        denied,
        Err(CoreError::SchemaViolation(error))
            if matches!(error.kind(), ViolationKind::IntegerAboveMaximum { .. })
    ));
    let snapshot = core.read(&all());
    assert_eq!(snapshot.sequence(), sequence);
    assert!(snapshot.get(&topic("/unrelated")).is_none());
}

#[test]
fn warning_rules_accept_the_original_value_and_return_a_diagnostic() {
    let mut core = Core::new();
    core.install_schema(
        integer_schema(Enforcement::Warn),
        SchemaInstallMode::RejectInvalid,
    )
    .unwrap();
    let outcome = core
        .apply(
            &actor(),
            batch(vec![WriteOperation::PublishState {
                topic: topic("/battery/phone"),
                value: Value::String("not a number".to_owned()),
                expiry: ExpiryUpdate::Clear,
            }]),
            now(),
        )
        .unwrap();
    assert!(matches!(
        outcome.warnings(),
        [Diagnostic::SchemaWarning(issue)]
            if matches!(issue.kind(), ViolationKind::CastFailed { target: ValueKind::Integer, .. })
    ));
}

#[test]
fn schema_install_does_not_cast_existing_values_and_force_cleans_invalid_state() {
    let actor = actor();
    let mut core = Core::new();
    core.apply(
        &actor,
        batch(vec![WriteOperation::PublishState {
            topic: topic("/battery/phone"),
            value: Value::String("42".to_owned()),
            expiry: ExpiryUpdate::Clear,
        }]),
        now(),
    )
    .unwrap();

    let rejected = core.install_schema(
        integer_schema(Enforcement::Deny),
        SchemaInstallMode::RejectInvalid,
    );
    assert!(matches!(
        rejected,
        Err(CoreError::ExistingSchemaViolations { .. })
    ));
    assert!(core.read(&all()).get(&topic("/battery/phone")).is_some());

    let forced = core
        .install_schema(
            integer_schema(Enforcement::Deny),
            SchemaInstallMode::RemoveInvalid,
        )
        .unwrap();
    assert!(forced.update().is_some());
    assert!(core.read(&all()).get(&topic("/battery/phone")).is_none());
}

#[test]
fn forced_schema_install_clears_desired_current_but_preserves_definition() {
    let actor = actor();
    let path = topic("/battery/target");
    let definition = InputDefinition::new();
    let mut core = Core::new();
    core.apply(
        &actor,
        batch(vec![
            WriteOperation::DefineInput {
                topic: path.clone(),
                kind: InputKind::Desired,
                definition: definition.clone(),
            },
            WriteOperation::SubmitDesired {
                topic: path.clone(),
                value: Value::String("invalid".to_owned()),
                expiry: ExpiryUpdate::Clear,
            },
        ]),
        now(),
    )
    .unwrap();

    core.install_schema(
        integer_schema(Enforcement::Deny),
        SchemaInstallMode::RemoveInvalid,
    )
    .unwrap();
    let snapshot = core.read(&all());
    let Some(Node::Desired(desired)) = snapshot.get(&path) else {
        panic!("desired definition must remain");
    };
    assert_eq!(desired.definition(), &definition);
    assert!(desired.current().is_none());
}

#[test]
fn schema_can_deny_an_implicit_node_kind_change() {
    let path = topic("/battery/phone");
    let mut core = Core::new();
    core.install_schema(
        Schema::new(
            SchemaName::parse("battery kind").unwrap(),
            vec![
                SchemaRule::new(
                    Selector::parse("/battery/*").unwrap(),
                    Enforcement::Deny,
                    ValueValidator::any(NullPolicy::Allow),
                )
                .unwrap()
                .with_node_kind(NodeKind::State),
            ],
        )
        .unwrap(),
        SchemaInstallMode::RejectInvalid,
    )
    .unwrap();
    core.apply(
        &actor(),
        batch(vec![WriteOperation::PublishState {
            topic: path.clone(),
            value: Value::Integer(42),
            expiry: ExpiryUpdate::Clear,
        }]),
        now(),
    )
    .unwrap();

    let denied = core.apply(
        &actor(),
        batch(vec![WriteOperation::PublishEvent {
            topic: path.clone(),
            value: Value::Integer(42),
        }]),
        now(),
    );
    assert!(matches!(
        denied,
        Err(CoreError::SchemaViolation(error))
            if matches!(error.kind(), ViolationKind::WrongNodeKind {
                expected: NodeKind::State,
                actual: NodeKind::Event,
            })
    ));
    assert!(matches!(core.read(&all()).get(&path), Some(Node::State(_))));
}
