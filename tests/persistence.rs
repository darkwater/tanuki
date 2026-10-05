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
    core::Core,
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node,
        NonNegativeDuration, Selection, Selector, Timestamp, TopicPath, Value, WriteBatch,
        WriteContext, WriteOperation,
    },
    persistence::{PersistenceError, SnapshotStore},
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
    let store = SnapshotStore::new(directory.path("snapshot.json"));
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

#[test]
fn missing_is_empty_but_malformed_and_unknown_versions_are_visible_errors() {
    let directory = TestDirectory::new();
    let path = directory.path("snapshot.json");
    let store = SnapshotStore::new(path.clone());
    assert!(store.load(at(0)).unwrap().is_none());

    fs::write(&path, b"{").unwrap();
    assert!(matches!(
        store.load(at(0)),
        Err(PersistenceError::Decode { .. })
    ));
    fs::write(
        &path,
        br#"{"format":"tanuki-snapshot","version":99,"sequence":0,"saved_at":"1970-01-01T00:00:00Z","nodes":{}}"#,
    )
    .unwrap();
    assert!(matches!(
        store.load(at(0)),
        Err(PersistenceError::Unsupported { version: 99, .. })
    ));
}

#[tokio::test]
async fn failed_atomic_replacement_is_returned_without_destroying_target() {
    let directory = TestDirectory::new();
    let target = directory.path("snapshot.json");
    fs::create_dir(&target).unwrap();
    let store = SnapshotStore::new(target.clone());
    let result = store.save(Arc::new(Mutex::new(Core::new())), at(0)).await;
    assert!(matches!(result, Err(PersistenceError::Write { .. })));
    assert!(target.is_dir());
}
