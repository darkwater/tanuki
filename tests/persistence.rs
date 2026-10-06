use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use jiff::{SignedDuration, Timestamp as JiffTimestamp};
use tanuki::{
    core::{Core, CoreError, SchemaInstallMode},
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node,
        NonNegativeDuration, Selection, Selector, Timestamp, TopicPath, Value, WriteBatch,
        WriteContext, WriteOperation,
    },
    link::{LinkDefinition, LinkName},
    persistence::{PersistenceError, SnapshotStore},
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueValidator},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let unique = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tanuki-persistence-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn at(second: i64) -> Timestamp {
    Timestamp::new(JiffTimestamp::from_second(second).unwrap())
}

fn seconds(value: i64) -> NonNegativeDuration {
    NonNegativeDuration::new(SignedDuration::from_secs(value)).unwrap()
}

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

fn all() -> Selection {
    Selection::new(vec![Selector::parse("/**").unwrap()])
}

#[tokio::test]
async fn coherent_save_restore_filters_expired_values_and_clears_live_authority() {
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.path("snapshot.db"));
    let core = Arc::new(Mutex::new(Core::new()));
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
            &WriteContext::managed(session),
            WriteBatch::new(vec![
                WriteOperation::PublishState {
                    topic: topic("/state/live"),
                    value: Value::Integer(1),
                    expiry: ExpiryUpdate::Set(seconds(100)),
                },
                WriteOperation::PublishState {
                    topic: topic("/state/expired"),
                    value: Value::Integer(2),
                    expiry: ExpiryUpdate::Set(seconds(10)),
                },
                WriteOperation::PublishEvent {
                    topic: topic("/event/button"),
                    value: Value::String("pressed".to_owned()),
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
                    expiry: ExpiryUpdate::Set(seconds(100)),
                },
                WriteOperation::DefineInput {
                    topic: topic("/lamp/toggle"),
                    kind: InputKind::Command,
                    definition: InputDefinition::new(),
                },
            ])
            .unwrap(),
            at(0),
        )
        .unwrap();

    store.save(Arc::clone(&core), at(1)).await.unwrap();
    let bytes = fs::read(directory.path("snapshot.db")).unwrap();
    assert_ne!(bytes.first(), Some(&b'{'));
    assert_eq!(
        rmp_serde::from_slice::<serde_json::Value>(&bytes).unwrap()["format"],
        "tanuki-snapshot"
    );
    let mut restored = store.load(at(50)).unwrap().unwrap();
    assert_eq!(restored.managed_session_count(), 0);
    let snapshot = restored.read(&all());
    assert!(snapshot.nodes().contains_key(&topic("/state/live")));
    assert!(!snapshot.nodes().contains_key(&topic("/state/expired")));
    assert!(matches!(
        snapshot.nodes().get(&topic("/event/button")),
        Some(Node::Event(_))
    ));
    assert!(matches!(
        snapshot.nodes().get(&topic("/lamp/toggle")),
        Some(Node::Command(_))
    ));
    let Some(Node::Desired(desired)) = snapshot.nodes().get(&topic("/lamp/desired")) else {
        panic!("desired definition must restore");
    };
    assert!(desired.claim().is_none());
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(70));

    let sequence = snapshot.sequence().get();
    let outcome = restored
        .apply(
            &WriteContext::stateless(ClientName::parse("after restart").unwrap()),
            WriteBatch::new(vec![WriteOperation::PublishState {
                topic: topic("/state/after"),
                value: Value::Bool(true),
                expiry: ExpiryUpdate::Clear,
            }])
            .unwrap(),
            at(51),
        )
        .unwrap();
    assert_eq!(outcome.update().sequence().get(), sequence + 1);
}

#[tokio::test]
async fn installed_schemas_survive_restart_and_still_deny_invalid_writes() {
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.path("snapshot.db"));
    let mut core = Core::new();
    core.install_schema(
        Schema::new(
            SchemaName::parse("battery").unwrap(),
            vec![
                SchemaRule::new(
                    Selector::parse("/battery/*").unwrap(),
                    Enforcement::Deny,
                    ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap(),
                )
                .unwrap(),
            ],
        )
        .unwrap(),
        SchemaInstallMode::RejectInvalid,
        at(0),
    )
    .unwrap();
    store.save(Arc::new(Mutex::new(core)), at(1)).await.unwrap();

    let mut restored = store.load(at(2)).unwrap().unwrap();
    let result = restored.apply(
        &WriteContext::stateless(ClientName::parse("writer").unwrap()),
        WriteBatch::new(vec![WriteOperation::PublishState {
            topic: topic("/battery/phone"),
            value: Value::Integer(101),
            expiry: ExpiryUpdate::Clear,
        }])
        .unwrap(),
        at(2),
    );
    assert!(matches!(result, Err(CoreError::SchemaViolation(_))));
}

#[tokio::test]
async fn freshness_policy_survives_restart_and_restores_overdue_status() {
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.path("snapshot.db"));
    let mut core = Core::new();
    core.install_schema(
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
    core.apply(
        &WriteContext::stateless(ClientName::parse("sensor").unwrap()),
        WriteBatch::new(vec![WriteOperation::PublishState {
            topic: topic("/sensor/temperature"),
            value: Value::Integer(20),
            expiry: ExpiryUpdate::Clear,
        }])
        .unwrap(),
        at(0),
    )
    .unwrap();
    store.save(Arc::new(Mutex::new(core)), at(1)).await.unwrap();

    let restored = store.load(at(20)).unwrap().unwrap();
    let snapshot = restored.read(&all());
    assert!(snapshot.nodes().contains_key(&topic("/sensor/temperature")));
    assert!(
        snapshot
            .nodes()
            .contains_key(&topic("/$diagnostics/freshness/sensor/temperature"))
    );
}

#[tokio::test]
async fn installed_links_survive_restart_without_persisting_alias_copies() {
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.path("snapshot.db"));
    let mut core = Core::new();
    core.apply(
        &WriteContext::stateless(ClientName::parse("writer").unwrap()),
        WriteBatch::new(vec![WriteOperation::PublishState {
            topic: topic("/devices/lamp/state"),
            value: Value::Bool(false),
            expiry: ExpiryUpdate::Clear,
        }])
        .unwrap(),
        at(0),
    )
    .unwrap();
    core.install_link(
        LinkDefinition::new(
            LinkName::parse("living-room").unwrap(),
            topic("/rooms/living-room"),
            topic("/devices/lamp"),
        )
        .unwrap(),
        at(0),
    )
    .unwrap();
    store.save(Arc::new(Mutex::new(core)), at(1)).await.unwrap();

    let mut restored = store.load(at(2)).unwrap().unwrap();
    assert_eq!(restored.link_count(), 1);
    assert!(
        restored
            .read(&all())
            .nodes()
            .contains_key(&topic("/rooms/living-room/state"))
    );
    restored
        .apply(
            &WriteContext::stateless(ClientName::parse("restored writer").unwrap()),
            WriteBatch::new(vec![WriteOperation::PublishState {
                topic: topic("/rooms/living-room/state"),
                value: Value::Bool(true),
                expiry: ExpiryUpdate::Clear,
            }])
            .unwrap(),
            at(2),
        )
        .unwrap();
    let snapshot = restored.read(&all());
    let Some(Node::State(state)) = snapshot.nodes().get(&topic("/devices/lamp/state")) else {
        panic!("canonical state must remain the persisted owner");
    };
    assert_eq!(state.current().value(), &Value::Bool(true));
}

#[test]
fn missing_is_empty_but_malformed_and_unknown_versions_are_visible_errors() {
    let directory = TestDirectory::new();
    let path = directory.path("snapshot.db");
    let store = SnapshotStore::new(path.clone());
    assert!(store.load(at(0)).unwrap().is_none());

    fs::write(&path, b"{").unwrap();
    assert!(matches!(
        store.load(at(0)),
        Err(PersistenceError::Decode { .. })
    ));
    fs::write(
        &path,
        rmp_serde::to_vec_named(&serde_json::json!({
            "format": "tanuki-snapshot",
            "version": 99,
            "sequence": 0,
            "saved_at": "1970-01-01T00:00:00Z",
            "nodes": {}
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        store.load(at(0)),
        Err(PersistenceError::Unsupported { version: 99, .. })
    ));
}

#[test]
fn corrupt_snapshot_is_backed_up_before_recovery_starts_empty() {
    let directory = TestDirectory::new();
    let path = directory.path("tanuki.db");
    fs::write(&path, b"not messagepack").unwrap();
    let store = SnapshotStore::new(path.clone());

    let recovered = store.load_or_recover(at(0)).unwrap();
    assert!(recovered.was_recovered());
    assert!(recovered.into_core().read(&all()).nodes().is_empty());
    assert_eq!(
        fs::read(path.with_extension("db.bak")).unwrap(),
        b"not messagepack"
    );

    let recovered = store.load_or_recover(at(0)).unwrap();
    assert!(recovered.was_recovered());
    assert_eq!(
        fs::read(directory.path("tanuki.db.bak.1")).unwrap(),
        b"not messagepack"
    );
}

#[tokio::test]
async fn failed_atomic_replacement_is_returned_without_destroying_target() {
    let directory = TestDirectory::new();
    let target = directory.path("snapshot.db");
    fs::create_dir(&target).unwrap();
    let store = SnapshotStore::new(target.clone());
    let result = store.save(Arc::new(Mutex::new(Core::new())), at(0)).await;
    assert!(matches!(result, Err(PersistenceError::Write { .. })));
    assert!(target.is_dir());
}

#[tokio::test]
async fn binary_semantic_and_tag_looking_values_survive_a_snapshot_round_trip() {
    use std::collections::BTreeMap;
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.path("values.db"));
    let values = [
        Value::Bytes(vec![0, 255]),
        Value::Timestamp("2023-11-14T22:13:20.123456789Z".parse().unwrap()),
        Value::Duration("-PT5.125S".parse().unwrap()),
        Value::Map(BTreeMap::from([(
            "$bytes".into(),
            Value::String("literal".into()),
        )])),
    ];
    let core = Arc::new(Mutex::new(Core::new()));
    core.lock()
        .unwrap()
        .apply(
            &WriteContext::stateless(ClientName::parse("codec publisher").unwrap()),
            WriteBatch::new(
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| WriteOperation::PublishState {
                        topic: topic(&format!("/values/{index}")),
                        value: value.clone(),
                        expiry: ExpiryUpdate::Clear,
                    })
                    .collect(),
            )
            .unwrap(),
            at(0),
        )
        .unwrap();
    store.save(core, at(1)).await.unwrap();
    let restored = store.load(at(2)).unwrap().unwrap();
    let snapshot = restored.read(&all());
    for (index, value) in values.iter().enumerate() {
        assert_eq!(
            snapshot.nodes()[&topic(&format!("/values/{index}"))]
                .retained_value()
                .unwrap()
                .value(),
            value
        );
    }
}
