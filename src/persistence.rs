use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    core::{CommitSequence, Core},
    domain::{
        ClientName, CommandNode, Deadline, DesiredNode, EventNode, InputDefinition, Node,
        RetainedValue, Selection, Selector, StateNode, Timestamp, TopicPath, WriteContext,
        WriteProvenance,
    },
    protocol::JsonValue,
};

const FORMAT: &str = "tanuki-snapshot";
const VERSION: u32 = 1;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct SnapshotStore {
    path: PathBuf,
}

impl SnapshotStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self, now: Timestamp) -> Result<Option<Core>, PersistenceError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(PersistenceError::Read { source }),
        };
        SnapshotFile::decode(&bytes)?.restore(now).map(Some)
    }

    pub fn load_or_recover(&self, now: Timestamp) -> Result<LoadOutcome, PersistenceError> {
        match self.load(now) {
            Ok(core) => Ok(LoadOutcome {
                core: core.unwrap_or_else(Core::new),
                recovery: None,
            }),
            Err(error @ PersistenceError::Read { .. }) => Err(error),
            Err(error) => {
                let backup = backup_corrupt_snapshot(&self.path)?;
                Ok(LoadOutcome {
                    core: Core::new(),
                    recovery: Some(SnapshotRecovery {
                        backup,
                        reason: error.to_string(),
                    }),
                })
            }
        }
    }

    pub async fn save(
        &self,
        core: Arc<Mutex<Core>>,
        now: Timestamp,
    ) -> Result<(), PersistenceError> {
        let snapshot = {
            let core = core.lock().map_err(|_| PersistenceError::CorePoisoned)?;
            SnapshotFile::capture(&core, now)?
        };
        let bytes = rmp_serde::to_vec_named(&snapshot)
            .map_err(|source| PersistenceError::Encode { source })?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write_atomic(&path, &bytes))
            .await
            .map_err(|source| PersistenceError::Task { source })??;
        Ok(())
    }
}

#[derive(Debug)]
pub struct LoadOutcome {
    core: Core,
    recovery: Option<SnapshotRecovery>,
}

impl LoadOutcome {
    #[must_use]
    pub fn was_recovered(&self) -> bool {
        self.recovery.is_some()
    }

    #[must_use]
    pub fn recovery(&self) -> Option<&SnapshotRecovery> {
        self.recovery.as_ref()
    }

    #[must_use]
    pub fn into_core(self) -> Core {
        self.core
    }
}

#[derive(Debug)]
pub struct SnapshotRecovery {
    backup: PathBuf,
    reason: String,
}

impl SnapshotRecovery {
    #[must_use]
    pub fn backup(&self) -> &Path {
        &self.backup
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("failed to read the snapshot")]
    Read {
        #[source]
        source: io::Error,
    },
    #[error("failed to decode the snapshot")]
    Decode {
        #[source]
        source: rmp_serde::decode::Error,
    },
    #[error("unsupported snapshot format `{format}` version {version}")]
    Unsupported { format: String, version: u32 },
    #[error("invalid timestamp `{value}` in snapshot")]
    Timestamp {
        value: String,
        #[source]
        source: jiff::Error,
    },
    #[error("failed to encode the snapshot")]
    Encode {
        #[source]
        source: rmp_serde::encode::Error,
    },
    #[error("failed to back up the corrupt snapshot")]
    Backup {
        #[source]
        source: io::Error,
    },
    #[error("failed to write the snapshot")]
    Write {
        #[source]
        source: io::Error,
    },
    #[error("snapshot task failed")]
    Task {
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("core state lock was poisoned")]
    CorePoisoned,
}

#[derive(Serialize, Deserialize)]
struct SnapshotFile {
    format: String,
    version: u32,
    sequence: u64,
    saved_at: String,
    nodes: BTreeMap<TopicPath, StoredNode>,
}

impl SnapshotFile {
    fn capture(core: &Core, now: Timestamp) -> Result<Self, PersistenceError> {
        let all = Selection::new(vec![
            Selector::parse("/**").expect("root selector is valid"),
        ]);
        let snapshot = core.read(&all);
        let nodes = snapshot
            .nodes()
            .iter()
            .map(|(topic, node)| Ok((topic.clone(), StoredNode::capture(node))))
            .collect::<Result<_, PersistenceError>>()?;
        Ok(Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            sequence: snapshot.sequence().get(),
            saved_at: now.get().to_string(),
            nodes,
        })
    }

    fn decode(bytes: &[u8]) -> Result<Self, PersistenceError> {
        let snapshot: Self =
            rmp_serde::from_slice(bytes).map_err(|source| PersistenceError::Decode { source })?;
        if snapshot.format != FORMAT || snapshot.version != VERSION {
            return Err(PersistenceError::Unsupported {
                format: snapshot.format,
                version: snapshot.version,
            });
        }
        Ok(snapshot)
    }

    fn restore(self, now: Timestamp) -> Result<Core, PersistenceError> {
        parse_timestamp(&self.saved_at)?;
        let nodes = self
            .nodes
            .into_iter()
            .filter_map(|(topic, node)| {
                node.restore(now)
                    .transpose()
                    .map(|node| node.map(|node| (topic, node)))
            })
            .collect::<Result<_, _>>()?;
        Ok(Core::from_restored(
            CommitSequence::from_persisted(self.sequence),
            nodes,
        ))
    }
}

fn backup_corrupt_snapshot(path: &Path) -> Result<PathBuf, PersistenceError> {
    let mut source = File::open(path).map_err(|source| PersistenceError::Backup { source })?;
    let mut suffix = 0_u64;
    loop {
        let mut name = path.as_os_str().to_os_string();
        if suffix == 0 {
            name.push(".bak");
        } else {
            name.push(format!(".bak.{suffix}"));
        }
        let backup = PathBuf::from(name);
        let mut destination = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                suffix += 1;
                continue;
            }
            Err(source) => return Err(PersistenceError::Backup { source }),
        };
        let result = io::copy(&mut source, &mut destination)
            .and_then(|_| destination.sync_all())
            .map_err(|source| PersistenceError::Backup { source });
        if let Err(error) = result {
            let _ = fs::remove_file(&backup);
            return Err(error);
        }
        return Ok(backup);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredNode {
    State { current: StoredCurrent },
    Event { last_publisher: StoredProvenance },
    Desired { current: Option<StoredCurrent> },
    Command,
}

impl StoredNode {
    fn capture(node: &Node) -> Self {
        match node {
            Node::State(node) => Self::State {
                current: StoredCurrent::capture(node.current()),
            },
            Node::Event(node) => Self::Event {
                last_publisher: StoredProvenance::capture(node.last_publisher()),
            },
            Node::Desired(node) => Self::Desired {
                current: node.current().map(StoredCurrent::capture),
            },
            Node::Command(_) => Self::Command,
        }
    }

    fn restore(self, now: Timestamp) -> Result<Option<Node>, PersistenceError> {
        Ok(match self {
            Self::State { current } => current
                .restore(now)?
                .map(|current| Node::State(StateNode::new(current))),
            Self::Event { last_publisher } => {
                Some(Node::Event(EventNode::new(last_publisher.restore()?)))
            }
            Self::Desired { current } => Some(Node::Desired(match current {
                Some(current) => match current.restore(now)? {
                    Some(current) => DesiredNode::with_current(InputDefinition::new(), current),
                    None => DesiredNode::new(InputDefinition::new()),
                },
                None => DesiredNode::new(InputDefinition::new()),
            })),
            Self::Command => Some(Node::Command(CommandNode::new(InputDefinition::new()))),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct StoredCurrent {
    value: JsonValue,
    provenance: StoredProvenance,
    expires_at: Option<String>,
}

impl StoredCurrent {
    fn capture(value: &RetainedValue) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
            provenance: StoredProvenance::capture(value.last_write()),
            expires_at: value
                .expires_at()
                .map(|value| value.get().get().to_string()),
        }
    }

    fn restore(self, now: Timestamp) -> Result<Option<RetainedValue>, PersistenceError> {
        let expires_at = self
            .expires_at
            .as_deref()
            .map(parse_timestamp)
            .transpose()?
            .map(Deadline::new);
        if expires_at.is_some_and(|deadline| deadline.get() <= now) {
            return Ok(None);
        }
        Ok(Some(RetainedValue::new(
            self.value.into_inner(),
            self.provenance.restore()?,
            expires_at,
        )))
    }
}

#[derive(Serialize, Deserialize)]
struct StoredProvenance {
    client: ClientName,
    at: String,
}

impl StoredProvenance {
    fn capture(value: &WriteProvenance) -> Self {
        Self {
            client: value.client().clone(),
            at: value.at().get().to_string(),
        }
    }

    fn restore(self) -> Result<WriteProvenance, PersistenceError> {
        Ok(WriteProvenance::from_context(
            &WriteContext::stateless(self.client),
            parse_timestamp(&self.at)?,
        ))
    }
}

fn parse_timestamp(value: &str) -> Result<Timestamp, PersistenceError> {
    value
        .parse()
        .map(Timestamp::new)
        .map_err(|source| PersistenceError::Timestamp {
            value: value.to_owned(),
            source,
        })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PersistenceError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|source| PersistenceError::Write { source })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snapshot");
    let unique = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{unique}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| PersistenceError::Write { source })?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| PersistenceError::Write { source })?;
        fs::rename(&temporary, path).map_err(|source| PersistenceError::Write { source })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| PersistenceError::Write { source })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
