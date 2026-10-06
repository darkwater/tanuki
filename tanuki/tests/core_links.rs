use tanuki::{
    core::{Change, Core, CoreError, Diagnostic, SchemaInstallMode},
    domain::{
        ClientName, ExpiryUpdate, Node, Selection, Selector, Timestamp, TopicPath, Value,
        WriteBatch, WriteContext, WriteOperation,
    },
    link::{LinkBuildError, LinkDefinition, LinkInstallError, LinkName},
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueCast, ValueValidator},
};

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn actor() -> WriteContext {
    WriteContext::stateless(ClientName::parse("test").unwrap())
}

fn now(second: i64) -> Timestamp {
    Timestamp::new(jiff::Timestamp::from_second(second).unwrap())
}

fn all() -> Selection {
    Selection::new(vec![Selector::parse("/**").unwrap()])
}

fn state(path: &str, value: Value) -> WriteOperation {
    WriteOperation::PublishState {
        topic: topic(path),
        value,
        expiry: ExpiryUpdate::Clear,
    }
}

fn batch(operations: Vec<WriteOperation>) -> WriteBatch {
    WriteBatch::new(operations).unwrap()
}

fn link() -> LinkDefinition {
    LinkDefinition::new(
        LinkName::parse("dashboard").unwrap(),
        topic("/view"),
        topic("/canonical"),
    )
    .unwrap()
}

fn range_schema(name: &str, selector: &str, cast: Option<ValueCast>) -> Schema {
    let rule = SchemaRule::new(
        Selector::parse(selector).unwrap(),
        Enforcement::Deny,
        ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap(),
    )
    .unwrap();
    let rule = match cast {
        Some(cast) => rule.with_cast(cast).unwrap(),
        None => rule,
    };
    Schema::new(SchemaName::parse(name).unwrap(), vec![rule]).unwrap()
}

#[test]
fn installed_subtree_link_projects_reads_and_canonical_updates() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![state("/canonical/device", Value::Integer(1))]),
        now(1),
    )
    .unwrap();
    let installed = core.install_link(link(), now(1)).unwrap();
    assert!(installed.enabled());

    let snapshot = core.read(&all());
    assert!(matches!(
        snapshot.get(&topic("/view/device")),
        Some(Node::State(_))
    ));

    let outcome = core
        .apply(
            &actor(),
            batch(vec![state("/canonical/device", Value::Integer(2))]),
            now(2),
        )
        .unwrap();
    assert!(outcome.update().changes().iter().any(|change| matches!(
        change,
        Change::Upsert { topic: changed, .. } if changed == &topic("/view/device")
    )));
}

#[test]
fn destination_collisions_are_rejected_without_installing_the_link() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![state("/view/existing", Value::Integer(1))]),
        now(1),
    )
    .unwrap();
    assert!(matches!(
        core.install_link(link(), now(1)),
        Err(CoreError::Link(_))
    ));
    assert_eq!(core.link_count(), 0);
}

#[test]
fn rejecting_alias_schema_disables_and_later_repairs_the_whole_view() {
    let mut core = Core::new();
    core.install_schema(
        range_schema("view range", "/view/*", None),
        tanuki::core::SchemaInstallMode::RejectInvalid,
        now(0),
    )
    .unwrap();
    core.apply(
        &actor(),
        batch(vec![state("/canonical/device", Value::Integer(50))]),
        now(1),
    )
    .unwrap();
    core.install_link(link(), now(1)).unwrap();

    let invalid = core
        .apply(
            &actor(),
            batch(vec![state("/canonical/device", Value::Integer(101))]),
            now(2),
        )
        .unwrap();
    assert!(matches!(
        invalid.warnings(),
        [Diagnostic::LinkDisabled { .. }]
    ));
    assert!(invalid.update().changes().iter().any(|change| matches!(
        change,
        Change::Removed { topic: changed, .. } if changed == &topic("/view/device")
    )));
    assert!(core.read(&all()).get(&topic("/view/device")).is_none());
    let condition = topic("/$diagnostics/links/dashboard");
    assert!(core.read(&all()).get(&condition).is_some());
    let Some(Node::State(canonical)) = core.read(&all()).get(&topic("/canonical/device")).cloned()
    else {
        panic!("canonical write must commit");
    };
    assert_eq!(canonical.current().value(), &Value::Integer(101));

    let repaired = core
        .apply(
            &actor(),
            batch(vec![state("/view/device", Value::Integer(42))]),
            now(3),
        )
        .unwrap();
    assert!(matches!(
        repaired.warnings(),
        [Diagnostic::LinkEnabled { .. }]
    ));
    assert!(repaired.update().changes().iter().any(|change| matches!(
        change,
        Change::Removed { topic, .. } if topic == &condition
    )));
    assert!(core.read(&all()).get(&topic("/view/device")).is_some());
    assert!(core.read(&all()).get(&condition).is_none());
}

#[test]
fn alias_schema_replacement_disables_and_reenables_existing_view() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![state("/canonical/device", Value::Integer(150))]),
        now(1),
    )
    .unwrap();
    core.install_link(link(), now(1)).unwrap();

    let disabled = core
        .install_schema(
            range_schema("view range", "/view/*", None),
            SchemaInstallMode::RejectInvalid,
            now(1),
        )
        .unwrap();
    assert!(matches!(
        disabled.warnings(),
        [Diagnostic::LinkDisabled { .. }]
    ));
    assert!(disabled.update().is_some());
    assert!(core.read(&all()).get(&topic("/view/device")).is_none());

    let enabled = core
        .install_schema(
            Schema::new(
                SchemaName::parse("view range").unwrap(),
                vec![
                    SchemaRule::new(
                        Selector::parse("/view/*").unwrap(),
                        Enforcement::Deny,
                        ValueValidator::integer_range(0, 200, NullPolicy::Deny).unwrap(),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
            SchemaInstallMode::RejectInvalid,
            now(2),
        )
        .unwrap();
    assert!(matches!(
        enabled.warnings(),
        [Diagnostic::LinkEnabled { .. }]
    ));
    assert!(core.read(&all()).get(&topic("/view/device")).is_some());
}

#[test]
fn topology_rejects_system_overlap_collisions_and_link_chains() {
    assert!(matches!(
        LinkDefinition::new(
            LinkName::parse("system").unwrap(),
            topic("/$view"),
            topic("/canonical"),
        ),
        Err(LinkBuildError::SystemMount { .. })
    ));
    assert!(matches!(
        LinkDefinition::new(
            LinkName::parse("self").unwrap(),
            topic("/canonical/view"),
            topic("/canonical"),
        ),
        Err(LinkBuildError::OverlappingSourceAndMount { .. })
    ));

    let mut core = Core::new();
    core.install_link(
        LinkDefinition::new(
            LinkName::parse("first").unwrap(),
            topic("/view"),
            topic("/canonical"),
        )
        .unwrap(),
        now(0),
    )
    .unwrap();
    assert!(matches!(
        core.install_link(
            LinkDefinition::new(
                LinkName::parse("overlap").unwrap(),
                topic("/view/nested"),
                topic("/other"),
            )
            .unwrap(),
            now(0),
        ),
        Err(CoreError::Link(LinkInstallError::OverlappingMounts { .. }))
    ));
    assert!(matches!(
        core.install_link(
            LinkDefinition::new(
                LinkName::parse("chain").unwrap(),
                topic("/other-view"),
                topic("/view/source"),
            )
            .unwrap(),
            now(0),
        ),
        Err(CoreError::Link(LinkInstallError::TargetThroughLink { .. }))
    ));
    assert_eq!(core.link_count(), 1);
}

#[test]
fn replacing_and_removing_a_link_retracts_only_its_old_projection() {
    let mut core = Core::new();
    core.apply(
        &actor(),
        batch(vec![
            state("/canonical/first", Value::Integer(1)),
            state("/other/second", Value::Integer(2)),
        ]),
        now(1),
    )
    .unwrap();
    core.install_link(link(), now(1)).unwrap();
    let replacement = LinkDefinition::new(
        LinkName::parse("dashboard").unwrap(),
        topic("/alternate"),
        topic("/other"),
    )
    .unwrap();
    let replaced = core.install_link(replacement, now(1)).unwrap();
    let changes = replaced.update().unwrap().changes();
    assert!(changes.iter().any(|change| matches!(
        change,
        Change::Removed { topic: changed, .. } if changed == &topic("/view/first")
    )));
    assert!(changes.iter().any(|change| matches!(
        change,
        Change::Upsert { topic: changed, .. } if changed == &topic("/alternate/second")
    )));

    let removed = core
        .remove_link(&LinkName::parse("dashboard").unwrap(), now(1))
        .unwrap();
    assert!(removed.removed());
    assert!(
        removed
            .update()
            .unwrap()
            .changes()
            .iter()
            .any(|change| matches!(
                change,
                Change::Removed { topic: changed, .. } if changed == &topic("/alternate/second")
            ))
    );
    assert_eq!(core.link_count(), 0);
    assert!(
        !core
            .remove_link(&LinkName::parse("dashboard").unwrap(), now(1))
            .unwrap()
            .removed()
    );
}

#[test]
fn alias_write_casts_at_the_view_then_validates_and_commits_canonically() {
    let mut core = Core::new();
    core.install_schema(
        range_schema("view cast", "/view/*", Some(ValueCast::StringToInteger)),
        tanuki::core::SchemaInstallMode::RejectInvalid,
        now(0),
    )
    .unwrap();
    core.install_schema(
        range_schema("canonical range", "/canonical/*", None),
        tanuki::core::SchemaInstallMode::RejectInvalid,
        now(0),
    )
    .unwrap();
    core.install_link(link(), now(0)).unwrap();

    core.apply(
        &actor(),
        batch(vec![state("/view/device", Value::String("42".to_owned()))]),
        now(1),
    )
    .unwrap();
    let snapshot = core.read(&all());
    let Some(Node::State(canonical)) = snapshot.get(&topic("/canonical/device")) else {
        panic!("alias write must create the canonical state");
    };
    assert_eq!(canonical.current().value(), &Value::Integer(42));
}

#[test]
fn invalid_event_is_canonical_only_and_later_valid_event_recovers_without_replay() {
    let mut core = Core::new();
    core.install_schema(
        range_schema("view range", "/view/*", None),
        tanuki::core::SchemaInstallMode::RejectInvalid,
        now(0),
    )
    .unwrap();
    core.install_link(link(), now(0)).unwrap();

    let invalid = core
        .apply(
            &actor(),
            batch(vec![WriteOperation::PublishEvent {
                topic: topic("/canonical/button"),
                value: Value::Integer(101),
            }]),
            now(1),
        )
        .unwrap();
    assert_eq!(
        invalid
            .update()
            .changes()
            .iter()
            .filter(|change| matches!(change, Change::Occurrence { .. }))
            .count(),
        1
    );
    assert!(matches!(
        invalid.warnings(),
        [Diagnostic::LinkDisabled { .. }]
    ));

    let valid = core
        .apply(
            &actor(),
            batch(vec![WriteOperation::PublishEvent {
                topic: topic("/canonical/button"),
                value: Value::Integer(1),
            }]),
            now(2),
        )
        .unwrap();
    assert_eq!(
        valid
            .update()
            .changes()
            .iter()
            .filter(|change| matches!(change, Change::Occurrence { .. }))
            .count(),
        2
    );
    assert!(matches!(valid.warnings(), [Diagnostic::LinkEnabled { .. }]));
}
