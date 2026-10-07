//! Commit views, outcomes, identities, and typed core errors.
use super::*;

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("canonical restored topic `{topic}` is in the reserved system namespace")]
    SystemTopic { topic: TopicPath },
    #[error("restored data violates its installed schema: {issue}")]
    SchemaViolation { issue: SchemaIssue },
    #[error(transparent)]
    Link(#[from] LinkInstallError),
}

/// Owned coherent input to persistence, independent of locks and disk codecs.
#[derive(Debug)]
pub struct PersistenceSnapshot {
    pub(crate) sequence: CommitSequence,
    pub(crate) nodes: BTreeMap<TopicPath, Node>,
    pub(crate) schemas: SchemaRegistry,
    pub(crate) links: Vec<LinkDefinition>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitSequence(pub(super) u64);

impl CommitSequence {
    pub(crate) const fn from_persisted(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SubscriptionId(pub(super) u64);

impl SubscriptionId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubscriptionCapacity(NonZeroUsize);

impl SubscriptionCapacity {
    pub fn new(value: usize) -> Result<Self, InvalidSubscriptionCapacity> {
        NonZeroUsize::new(value)
            .map(Self)
            .ok_or(InvalidSubscriptionCapacity)
    }

    pub(super) const fn get(self) -> usize {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("a subscription queue must hold at least one update batch")]
pub struct InvalidSubscriptionCapacity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionEnd {
    SlowConsumer,
}

#[derive(Debug)]
pub(super) struct SubscriberState {
    pub(super) selection: Selection,
    pub(super) updates: mpsc::Sender<UpdateBatch>,
    pub(super) end: Arc<Mutex<Option<SubscriptionEnd>>>,
}

#[derive(Debug)]
pub struct Subscription {
    pub(super) id: SubscriptionId,
    pub(super) snapshot: Snapshot,
    pub(super) updates: mpsc::Receiver<UpdateBatch>,
    pub(super) end: Arc<Mutex<Option<SubscriptionEnd>>>,
}

impl Subscription {
    #[must_use]
    pub const fn id(&self) -> SubscriptionId {
        self.id
    }

    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn try_update(&mut self) -> Option<UpdateBatch> {
        self.updates.try_recv().ok()
    }

    pub async fn update(&mut self) -> Option<UpdateBatch> {
        self.updates.recv().await
    }

    #[must_use]
    pub fn end_reason(&self) -> Option<SubscriptionEnd> {
        *self
            .end
            .lock()
            .expect("subscription end lock is not poisoned")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub(super) sequence: CommitSequence,
    pub(super) nodes: BTreeMap<TopicPath, Node>,
}

impl Snapshot {
    #[must_use]
    pub const fn sequence(&self) -> CommitSequence {
        self.sequence
    }

    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<TopicPath, Node> {
        &self.nodes
    }

    #[must_use]
    pub fn get(&self, topic: &TopicPath) -> Option<&Node> {
        self.nodes.get(topic)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommitOutcome {
    pub(super) update: UpdateBatch,
    pub(super) warnings: Vec<Diagnostic>,
}

impl CommitOutcome {
    #[must_use]
    pub fn update(&self) -> &UpdateBatch {
        &self.update
    }

    #[must_use]
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateBatch {
    pub(super) sequence: CommitSequence,
    pub(super) changes: Vec<Change>,
}

impl UpdateBatch {
    #[must_use]
    pub const fn sequence(&self) -> CommitSequence {
        self.sequence
    }

    #[must_use]
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Upsert {
        topic: TopicPath,
        node: Node,
    },
    Occurrence {
        topic: TopicPath,
        event: EventOccurrence,
    },
    Command {
        topic: TopicPath,
        command: CommandOccurrence,
    },
    Removed {
        topic: TopicPath,
        previous: Node,
    },
}

impl Change {
    pub(super) fn topic(&self) -> &TopicPath {
        match self {
            Self::Upsert { topic, .. }
            | Self::Occurrence { topic, .. }
            | Self::Command { topic, .. }
            | Self::Removed { topic, .. } => topic,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Diagnostic {
    OutputOwnerChanged {
        topic: TopicPath,
        previous: ClientName,
        replacement: ClientName,
    },
    NodeKindChanged {
        topic: TopicPath,
        previous: NodeKind,
        replacement: NodeKind,
    },
    NodeAlreadyAbsent {
        topic: TopicPath,
    },
    SessionReplaced {
        client: ClientName,
        previous: SessionId,
        replacement: SessionId,
    },
    InputClaimReplaced {
        topic: TopicPath,
        previous: ClientName,
        replacement: ClientName,
    },
    ClaimGraceOutOfRange {
        topic: TopicPath,
    },
    SchemaWarning(SchemaIssue),
    LinkDisabled {
        link: LinkName,
        issue: SchemaIssue,
    },
    LinkEnabled {
        link: LinkName,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkInstallOutcome {
    pub(super) enabled: bool,
    pub(super) warnings: Vec<Diagnostic>,
    pub(super) update: Option<UpdateBatch>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkRemovalOutcome {
    pub(super) removed: bool,
    pub(super) update: Option<UpdateBatch>,
}

impl LinkRemovalOutcome {
    #[must_use]
    pub const fn removed(&self) -> bool {
        self.removed
    }

    #[must_use]
    pub fn update(&self) -> Option<&UpdateBatch> {
        self.update.as_ref()
    }
}

impl LinkInstallOutcome {
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    #[must_use]
    pub fn update(&self) -> Option<&UpdateBatch> {
        self.update.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaInstallMode {
    RejectInvalid,
    RemoveInvalid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaInstallOutcome {
    pub(super) warnings: Vec<Diagnostic>,
    pub(super) update: Option<UpdateBatch>,
}

impl SchemaInstallOutcome {
    #[must_use]
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    #[must_use]
    pub fn update(&self) -> Option<&UpdateBatch> {
        self.update.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenSessionOutcome {
    pub(super) handle: SessionHandle,
    pub(super) warnings: Vec<Diagnostic>,
    pub(super) update: Option<UpdateBatch>,
    pub(super) pending_releases: Vec<PendingClaimRelease>,
}

impl OpenSessionOutcome {
    #[must_use]
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    #[must_use]
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    #[must_use]
    pub fn update(&self) -> Option<&UpdateBatch> {
        self.update.as_ref()
    }

    #[must_use]
    pub fn pending_releases(&self) -> &[PendingClaimRelease] {
        &self.pending_releases
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisconnectOutcome {
    pub(super) was_current: bool,
    pub(super) warnings: Vec<Diagnostic>,
    pub(super) update: Option<UpdateBatch>,
    pub(super) pending_releases: Vec<PendingClaimRelease>,
}

impl DisconnectOutcome {
    #[must_use]
    pub const fn was_current(&self) -> bool {
        self.was_current
    }

    #[must_use]
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    #[must_use]
    pub fn update(&self) -> Option<&UpdateBatch> {
        self.update.as_ref()
    }

    #[must_use]
    pub fn pending_releases(&self) -> &[PendingClaimRelease] {
        &self.pending_releases
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingClaimRelease {
    pub(super) topic: TopicPath,
    pub(super) claim: ClaimId,
    pub(super) deadline: Deadline,
}

impl PendingClaimRelease {
    #[must_use]
    pub fn topic(&self) -> &TopicPath {
        &self.topic
    }

    #[must_use]
    pub const fn claim(&self) -> ClaimId {
        self.claim
    }

    #[must_use]
    pub const fn deadline(&self) -> Deadline {
        self.deadline
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("writes to reserved system topic `{topic}` are not permitted")]
    SystemTopic { topic: TopicPath },
    #[error("operation `{operation}` is not implemented in the current core phase")]
    UnsupportedOperation { operation: &'static str },
    #[error("deadline for `{topic}` is outside the supported timestamp range")]
    DeadlineOutOfRange {
        topic: TopicPath,
        #[source]
        source: jiff::Error,
    },
    #[error("the in-memory commit sequence is exhausted")]
    SequenceExhausted,
    #[error("managed session identifiers are exhausted")]
    SessionIdExhausted,
    #[error("input claim identifiers are exhausted")]
    ClaimIdExhausted,
    #[error("subscription identifiers are exhausted")]
    SubscriptionIdExhausted,
    #[error("managed session {session:?} is no longer current")]
    SessionExpired { session: SessionId },
    #[error("operation `{operation}` requires a managed session")]
    SessionRequired { operation: &'static str },
    #[error("input `{topic}` is not defined")]
    InputNotDefined { topic: TopicPath },
    #[error("node `{topic}` has non-input kind {actual:?}")]
    NotAnInput { topic: TopicPath, actual: NodeKind },
    #[error("input `{topic}` has kind {actual:?}, expected {expected:?}")]
    WrongInputKind {
        topic: TopicPath,
        expected: InputKind,
        actual: NodeKind,
    },
    #[error("input definition `{topic}` is controlled by another claim")]
    ClaimAuthorityRequired { topic: TopicPath },
    #[error(transparent)]
    SchemaViolation(#[from] SchemaViolation),
    #[error(transparent)]
    SchemaRegistry(#[from] SchemaRegistryError),
    #[error("existing values violate the proposed schema")]
    ExistingSchemaViolations { violations: Vec<SchemaIssue> },
    #[error(transparent)]
    Link(#[from] LinkInstallError),
}
