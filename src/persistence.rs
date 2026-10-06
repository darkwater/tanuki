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
        ClientName, CommandNode, Deadline, DesiredNode, EventNode, FiniteF64, InputDefinition,
        Node, NodeKind, NonNegativeDuration, RetainedValue, Selector, SelectorParseError,
        StateNode, Timestamp, TopicPath, ValueKind, WriteContext, WriteProvenance,
    },
    link::{LinkBuildError, LinkDefinition, LinkInstallError, LinkName, LinkNameParseError},
    protocol::JsonValue,
    schema::{
        Enforcement, NullPolicy, Schema, SchemaBuildError, SchemaIssue, SchemaName,
        SchemaNameParseError, SchemaRegistry, SchemaRegistryError, SchemaRule,
        SchemaRuleBuildError, ValidatorBuildError, ValidatorShape, ValueCast, ValueValidator,
    },
};

const FORMAT: &str = "tanuki-snapshot";
const VERSION: u32 = 3;
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
    #[error("invalid schema in snapshot")]
    Schema {
        #[source]
        source: SchemaPersistenceError,
    },
    #[error("invalid link in snapshot")]
    Link {
        #[source]
        source: LinkPersistenceError,
    },
}

#[derive(Debug, Error)]
pub enum SchemaPersistenceError {
    #[error(transparent)]
    Name(#[from] SchemaNameParseError),
    #[error(transparent)]
    Selector(#[from] SelectorParseError),
    #[error(transparent)]
    Validator(#[from] ValidatorBuildError),
    #[error(transparent)]
    Rule(#[from] SchemaRuleBuildError),
    #[error(transparent)]
    Schema(#[from] SchemaBuildError),
    #[error(transparent)]
    Registry(#[from] SchemaRegistryError),
    #[error("schema float bound {value} is not finite")]
    NonFiniteFloat { value: f64 },
    #[error("restored data violates its installed schema: {issue}")]
    ExistingValue { issue: SchemaIssue },
    #[error("invalid expected update interval `{value}` in snapshot")]
    FreshnessInterval {
        value: String,
        #[source]
        source: jiff::Error,
    },
    #[error("expected update interval `{value}` in snapshot is negative")]
    NegativeFreshnessInterval { value: String },
}

#[derive(Debug, Error)]
pub enum LinkPersistenceError {
    #[error(transparent)]
    Name(#[from] LinkNameParseError),
    #[error(transparent)]
    Definition(#[from] LinkBuildError),
    #[error(transparent)]
    Install(#[from] LinkInstallError),
}

#[derive(Serialize, Deserialize)]
struct SnapshotFile {
    format: String,
    version: u32,
    sequence: u64,
    saved_at: String,
    nodes: BTreeMap<TopicPath, StoredNode>,
    #[serde(default)]
    schemas: Vec<StoredSchema>,
    #[serde(default)]
    links: Vec<StoredLink>,
}

impl SnapshotFile {
    fn capture(core: &Core, now: Timestamp) -> Result<Self, PersistenceError> {
        let snapshot = core.persistence_snapshot();
        let nodes = snapshot
            .nodes()
            .iter()
            .map(|(topic, node)| Ok((topic.clone(), StoredNode::capture(node))))
            .collect::<Result<_, PersistenceError>>()?;
        let schemas = core
            .schemas()
            .schemas()
            .map(StoredSchema::capture)
            .collect();
        let links = core
            .links()
            .iter()
            .map(|link| StoredLink::capture(link.definition()))
            .collect();
        Ok(Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            sequence: snapshot.sequence().get(),
            saved_at: now.get().to_string(),
            nodes,
            schemas,
            links,
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
        let nodes: BTreeMap<TopicPath, Node> = self
            .nodes
            .into_iter()
            .filter_map(|(topic, node)| {
                node.restore(now)
                    .transpose()
                    .map(|node| node.map(|node| (topic, node)))
            })
            .collect::<Result<_, _>>()?;
        let mut schemas = SchemaRegistry::new();
        for schema in self.schemas {
            schemas
                .install(
                    schema
                        .restore()
                        .map_err(|source| PersistenceError::Schema { source })?,
                )
                .map_err(|source| PersistenceError::Schema {
                    source: source.into(),
                })?;
        }
        for (topic, node) in &nodes {
            let value = match node {
                Node::State(state) => Some(state.current().value()),
                Node::Desired(desired) => desired.current().map(RetainedValue::value),
                Node::Event(_) | Node::Command(_) => None,
            };
            if let Some((_, issue)) = schemas
                .inspect_existing(topic, node.kind(), value)
                .into_iter()
                .find(|(enforcement, _)| *enforcement == Enforcement::Deny)
            {
                return Err(PersistenceError::Schema {
                    source: SchemaPersistenceError::ExistingValue { issue },
                });
            }
        }
        let links = self
            .links
            .into_iter()
            .map(StoredLink::restore)
            .collect::<Result<_, _>>()
            .map_err(|source| PersistenceError::Link { source })?;
        Core::from_restored(
            CommitSequence::from_persisted(self.sequence),
            nodes,
            schemas,
            links,
            now,
        )
        .map_err(|source| PersistenceError::Link {
            source: source.into(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct StoredLink {
    name: String,
    mount: TopicPath,
    target: TopicPath,
}

impl StoredLink {
    fn capture(link: &LinkDefinition) -> Self {
        Self {
            name: link.name().as_str().to_owned(),
            mount: link.mount().clone(),
            target: link.target().clone(),
        }
    }

    fn restore(self) -> Result<LinkDefinition, LinkPersistenceError> {
        Ok(LinkDefinition::new(
            LinkName::parse(&self.name)?,
            self.mount,
            self.target,
        )?)
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSchema {
    name: String,
    rules: Vec<StoredSchemaRule>,
}

impl StoredSchema {
    fn capture(schema: &Schema) -> Self {
        Self {
            name: schema.name().as_str().to_owned(),
            rules: schema
                .rules()
                .iter()
                .map(StoredSchemaRule::capture)
                .collect(),
        }
    }

    fn restore(self) -> Result<Schema, SchemaPersistenceError> {
        let rules = self
            .rules
            .into_iter()
            .map(StoredSchemaRule::restore)
            .collect::<Result<_, _>>()?;
        Ok(Schema::new(SchemaName::parse(&self.name)?, rules)?)
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSchemaRule {
    selector: String,
    enforcement: StoredEnforcement,
    null_policy: StoredNullPolicy,
    validator: StoredValidator,
    cast: Option<StoredValueCast>,
    node_kind: Option<StoredNodeKind>,
    #[serde(default)]
    expected_update_interval: Option<String>,
}

impl StoredSchemaRule {
    fn capture(rule: &SchemaRule) -> Self {
        Self {
            selector: rule.selector().to_string(),
            enforcement: rule.enforcement().into(),
            null_policy: rule.validator().null_policy().into(),
            validator: StoredValidator::capture(rule.validator().shape()),
            cast: rule.cast().map(Into::into),
            node_kind: rule.node_kind().map(Into::into),
            expected_update_interval: rule.expected_update_interval().map(|interval| {
                jiff::fmt::temporal::SpanPrinter::new().duration_to_string(&interval.get())
            }),
        }
    }

    fn restore(self) -> Result<SchemaRule, SchemaPersistenceError> {
        let validator = self.validator.restore(self.null_policy.into())?;
        let rule = SchemaRule::new(
            Selector::parse(&self.selector)?,
            self.enforcement.into(),
            validator,
        )?;
        let rule = match self.cast {
            Some(cast) => rule.with_cast(cast.into())?,
            None => rule,
        };
        let rule = match self.node_kind {
            Some(node_kind) => rule.with_node_kind(node_kind.into()),
            None => rule,
        };
        let Some(value) = self.expected_update_interval else {
            return Ok(rule);
        };
        let duration =
            value
                .parse()
                .map_err(|source| SchemaPersistenceError::FreshnessInterval {
                    value: value.clone(),
                    source,
                })?;
        let interval = NonNegativeDuration::new(duration)
            .map_err(|_| SchemaPersistenceError::NegativeFreshnessInterval { value })?;
        Ok(rule.with_expected_update_interval(interval)?)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredEnforcement {
    Warn,
    Deny,
}

impl From<Enforcement> for StoredEnforcement {
    fn from(value: Enforcement) -> Self {
        match value {
            Enforcement::Warn => Self::Warn,
            Enforcement::Deny => Self::Deny,
        }
    }
}

impl From<StoredEnforcement> for Enforcement {
    fn from(value: StoredEnforcement) -> Self {
        match value {
            StoredEnforcement::Warn => Self::Warn,
            StoredEnforcement::Deny => Self::Deny,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredNullPolicy {
    Allow,
    Deny,
}

impl From<NullPolicy> for StoredNullPolicy {
    fn from(value: NullPolicy) -> Self {
        match value {
            NullPolicy::Allow => Self::Allow,
            NullPolicy::Deny => Self::Deny,
        }
    }
}

impl From<StoredNullPolicy> for NullPolicy {
    fn from(value: StoredNullPolicy) -> Self {
        match value {
            StoredNullPolicy::Allow => Self::Allow,
            StoredNullPolicy::Deny => Self::Deny,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredValueCast {
    Integer,
    Float,
    Bool,
}

impl From<ValueCast> for StoredValueCast {
    fn from(value: ValueCast) -> Self {
        match value {
            ValueCast::StringToInteger => Self::Integer,
            ValueCast::StringToFloat => Self::Float,
            ValueCast::StringToBool => Self::Bool,
        }
    }
}

impl From<StoredValueCast> for ValueCast {
    fn from(value: StoredValueCast) -> Self {
        match value {
            StoredValueCast::Integer => Self::StringToInteger,
            StoredValueCast::Float => Self::StringToFloat,
            StoredValueCast::Bool => Self::StringToBool,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredNodeKind {
    State,
    Event,
    Desired,
    Command,
}

impl From<NodeKind> for StoredNodeKind {
    fn from(value: NodeKind) -> Self {
        match value {
            NodeKind::State => Self::State,
            NodeKind::Event => Self::Event,
            NodeKind::Desired => Self::Desired,
            NodeKind::Command => Self::Command,
        }
    }
}

impl From<StoredNodeKind> for NodeKind {
    fn from(value: StoredNodeKind) -> Self {
        match value {
            StoredNodeKind::State => Self::State,
            StoredNodeKind::Event => Self::Event,
            StoredNodeKind::Desired => Self::Desired,
            StoredNodeKind::Command => Self::Command,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredValidator {
    Any,
    ValueKind { value_kind: StoredValueKind },
    IntegerRange { minimum: i64, maximum: i64 },
    FloatRange { minimum: f64, maximum: f64 },
    StringEnum { values: Vec<String> },
}

impl StoredValidator {
    fn capture(value: ValidatorShape<'_>) -> Self {
        match value {
            ValidatorShape::Any => Self::Any,
            ValidatorShape::Kind(value_kind) => Self::ValueKind {
                value_kind: value_kind.into(),
            },
            ValidatorShape::IntegerRange { minimum, maximum } => {
                Self::IntegerRange { minimum, maximum }
            }
            ValidatorShape::FloatRange { minimum, maximum } => Self::FloatRange {
                minimum: minimum.get(),
                maximum: maximum.get(),
            },
            ValidatorShape::StringEnum(values) => Self::StringEnum {
                values: values.iter().cloned().collect(),
            },
        }
    }

    fn restore(self, null_policy: NullPolicy) -> Result<ValueValidator, SchemaPersistenceError> {
        Ok(match self {
            Self::Any => ValueValidator::any(null_policy),
            Self::ValueKind { value_kind } => ValueValidator::kind(value_kind.into(), null_policy)?,
            Self::IntegerRange { minimum, maximum } => {
                ValueValidator::integer_range(minimum, maximum, null_policy)?
            }
            Self::FloatRange { minimum, maximum } => ValueValidator::float_range(
                FiniteF64::new(minimum)
                    .map_err(|_| SchemaPersistenceError::NonFiniteFloat { value: minimum })?,
                FiniteF64::new(maximum)
                    .map_err(|_| SchemaPersistenceError::NonFiniteFloat { value: maximum })?,
                null_policy,
            )?,
            Self::StringEnum { values } => ValueValidator::string_enum(values, null_policy)?,
        })
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredValueKind {
    Bool,
    Integer,
    Float,
    String,
    Bytes,
    List,
    Map,
    Timestamp,
    Duration,
}

impl From<ValueKind> for StoredValueKind {
    fn from(value: ValueKind) -> Self {
        match value {
            ValueKind::Null => unreachable!("null is represented by the null policy"),
            ValueKind::Bool => Self::Bool,
            ValueKind::Integer => Self::Integer,
            ValueKind::Float => Self::Float,
            ValueKind::String => Self::String,
            ValueKind::Bytes => Self::Bytes,
            ValueKind::List => Self::List,
            ValueKind::Map => Self::Map,
            ValueKind::Timestamp => Self::Timestamp,
            ValueKind::Duration => Self::Duration,
        }
    }
}

impl From<StoredValueKind> for ValueKind {
    fn from(value: StoredValueKind) -> Self {
        match value {
            StoredValueKind::Bool => Self::Bool,
            StoredValueKind::Integer => Self::Integer,
            StoredValueKind::Float => Self::Float,
            StoredValueKind::String => Self::String,
            StoredValueKind::Bytes => Self::Bytes,
            StoredValueKind::List => Self::List,
            StoredValueKind::Map => Self::Map,
            StoredValueKind::Timestamp => Self::Timestamp,
            StoredValueKind::Duration => Self::Duration,
        }
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

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredNode {
    State { current: StoredCurrent },
    Event { last_publisher: StoredProvenance },
    Desired { current: Option<StoredCurrent> },
    Command,
}

// Stream the fields so nested JsonValue sees the MessagePack decoder rather than
// Serde's human-readable ContentDeserializer. The stored format is unchanged.
impl<'de> Deserialize<'de> for StoredNode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            kind: String,
            current: Option<StoredCurrent>,
            last_publisher: Option<StoredProvenance>,
        }
        let fields = Fields::deserialize(deserializer)?;
        match fields.kind.as_str() {
            "state" => Ok(Self::State {
                current: fields
                    .current
                    .ok_or_else(|| serde::de::Error::missing_field("current"))?,
            }),
            "event" => Ok(Self::Event {
                last_publisher: fields
                    .last_publisher
                    .ok_or_else(|| serde::de::Error::missing_field("last_publisher"))?,
            }),
            "desired" => Ok(Self::Desired {
                current: fields.current,
            }),
            "command" => Ok(Self::Command),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["state", "event", "desired", "command"],
            )),
        }
    }
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
