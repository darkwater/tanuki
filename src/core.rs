use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use thiserror::Error;
use tokio::sync::mpsc;

use crate::domain::{
    ClaimId, ClaimRelease, ClientName, CommandNode, CommandOccurrence, Deadline, DesiredNode,
    EventNode, EventOccurrence, ExpiryUpdate, InputClaim, InputKind, Node, NodeKind, RetainedValue,
    Selection, SessionHandle, SessionId, StateNode, Timestamp, TopicPath, WriteBatch, WriteContext,
    WriteOperation, WriteProvenance,
};

#[derive(Debug, Default)]
pub struct Core {
    sequence: CommitSequence,
    nodes: BTreeMap<TopicPath, Node>,
    sessions: BTreeMap<ClientName, SessionId>,
    next_session_id: u64,
    next_claim_id: u64,
    subscribers: BTreeMap<SubscriptionId, SubscriberState>,
    next_subscription_id: u64,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_restored(
        sequence: CommitSequence,
        nodes: BTreeMap<TopicPath, Node>,
    ) -> Self {
        Self {
            sequence,
            nodes,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn read(&self, selection: &Selection) -> Snapshot {
        Snapshot {
            sequence: self.sequence,
            nodes: self
                .nodes
                .iter()
                .filter(|(topic, _)| selection.matches(topic))
                .map(|(topic, node)| (topic.clone(), node.clone()))
                .collect(),
        }
    }

    pub fn apply(
        &mut self,
        actor: &WriteContext,
        batch: WriteBatch,
        now: Timestamp,
    ) -> Result<CommitOutcome, CoreError> {
        self.validate_actor(actor)?;
        validate_topics(&batch)?;

        let next_sequence = self
            .sequence
            .0
            .checked_add(1)
            .map(CommitSequence)
            .ok_or(CoreError::SequenceExhausted)?;
        let previous = self.nodes.clone();
        let mut candidate = previous.clone();
        let mut next_claim_id = self.next_claim_id;
        let mut changes = Vec::new();
        let mut warnings = Vec::new();

        for operation in batch.operations() {
            apply_operation(
                &mut candidate,
                &mut changes,
                &mut warnings,
                actor,
                operation,
                now,
                &mut next_claim_id,
            )?;
        }

        let changes = coalesce_batch_changes(&previous, &candidate, changes);

        debug_assert!(candidate.keys().all(|topic| !topic.is_system()));
        self.nodes = candidate;
        self.next_claim_id = next_claim_id;
        self.sequence = next_sequence;
        let update = UpdateBatch {
            sequence: next_sequence,
            changes,
        };
        self.publish_update(&update);
        Ok(CommitOutcome { update, warnings })
    }

    pub fn open_session(
        &mut self,
        client: ClientName,
        now: Timestamp,
    ) -> Result<OpenSessionOutcome, CoreError> {
        let raw_id = self
            .next_session_id
            .checked_add(1)
            .ok_or(CoreError::SessionIdExhausted)?;
        let id = SessionId::new(raw_id);
        let previous = self.sessions.get(&client).copied();
        let mut candidate = self.nodes.clone();
        let mut warnings = Vec::new();
        let mut pending_releases = Vec::new();
        let mut changes = Vec::new();

        if let Some(previous) = previous {
            warnings.push(Diagnostic::SessionReplaced {
                client: client.clone(),
                previous,
                replacement: id,
            });
            collect_session_releases(
                &mut candidate,
                previous,
                now,
                &mut changes,
                &mut warnings,
                &mut pending_releases,
            );
        }

        let update = self.install_node_changes(candidate, changes)?;
        self.sessions.insert(client.clone(), id);
        self.next_session_id = raw_id;
        Ok(OpenSessionOutcome {
            handle: SessionHandle::new(id, client),
            warnings,
            update,
            pending_releases,
        })
    }

    pub fn disconnect(
        &mut self,
        handle: &SessionHandle,
        now: Timestamp,
    ) -> Result<DisconnectOutcome, CoreError> {
        if self.sessions.get(handle.client()) != Some(&handle.id()) {
            return Ok(DisconnectOutcome {
                was_current: false,
                warnings: Vec::new(),
                update: None,
                pending_releases: Vec::new(),
            });
        }

        let mut candidate = self.nodes.clone();
        let mut warnings = Vec::new();
        let mut pending_releases = Vec::new();
        let mut changes = Vec::new();
        collect_session_releases(
            &mut candidate,
            handle.id(),
            now,
            &mut changes,
            &mut warnings,
            &mut pending_releases,
        );
        let update = self.install_node_changes(candidate, changes)?;
        self.sessions.remove(handle.client());

        Ok(DisconnectOutcome {
            was_current: true,
            warnings,
            update,
            pending_releases,
        })
    }

    #[must_use]
    pub fn managed_session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn subscribe(
        &mut self,
        selection: Selection,
        capacity: SubscriptionCapacity,
    ) -> Result<Subscription, CoreError> {
        self.subscribers
            .retain(|_, subscriber| !subscriber.updates.is_closed());
        let raw_id = self
            .next_subscription_id
            .checked_add(1)
            .ok_or(CoreError::SubscriptionIdExhausted)?;
        let id = SubscriptionId(raw_id);
        let snapshot = self.read(&selection);
        let (updates, receiver) = mpsc::channel(capacity.get());
        let end = Arc::new(Mutex::new(None));
        self.subscribers.insert(
            id,
            SubscriberState {
                selection,
                updates,
                end: Arc::clone(&end),
            },
        );
        self.next_subscription_id = raw_id;
        Ok(Subscription {
            id,
            snapshot,
            updates: receiver,
            end,
        })
    }

    #[must_use]
    pub fn next_value_deadline(&self) -> Option<Deadline> {
        self.nodes
            .values()
            .filter_map(Node::retained_value)
            .filter_map(RetainedValue::expires_at)
            .min()
    }

    pub fn process_deadlines(
        &mut self,
        now: Timestamp,
        claim_releases: &[PendingClaimRelease],
    ) -> Result<Option<UpdateBatch>, CoreError> {
        let previous = self.nodes.clone();
        let mut candidate = previous.clone();
        let expired_topics = previous
            .iter()
            .filter(|(_, node)| {
                node.retained_value()
                    .and_then(RetainedValue::expires_at)
                    .is_some_and(|deadline| deadline.get() <= now)
            })
            .map(|(topic, _)| topic.clone())
            .collect::<Vec<_>>();

        for topic in expired_topics {
            match candidate.get(&topic).cloned() {
                Some(Node::State(_)) => {
                    candidate.remove(&topic);
                }
                Some(Node::Desired(desired)) => {
                    candidate.insert(
                        topic,
                        Node::Desired(DesiredNode::from_parts(
                            desired.definition().clone(),
                            desired.claim().cloned(),
                            None,
                        )),
                    );
                }
                Some(Node::Event(_) | Node::Command(_)) | None => {}
            }
        }

        for release in claim_releases
            .iter()
            .filter(|release| release.deadline().get() <= now)
        {
            let Some(node) = candidate.get_mut(release.topic()) else {
                continue;
            };
            if input_claim(Some(node)).is_some_and(|claim| claim.id() == release.claim()) {
                clear_claim(node);
            }
        }

        let changed_topics = previous
            .keys()
            .chain(candidate.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let changes = changed_topics
            .into_iter()
            .filter_map(
                |topic| match (previous.get(&topic), candidate.get(&topic)) {
                    (Some(old), Some(new)) if old != new => Some(Change::Upsert {
                        topic,
                        node: new.clone(),
                    }),
                    (Some(old), None) => Some(Change::Removed {
                        topic,
                        previous: old.clone(),
                    }),
                    _ => None,
                },
            )
            .collect();
        self.install_node_changes(candidate, changes)
    }

    fn validate_actor(&self, actor: &WriteContext) -> Result<(), CoreError> {
        let WriteContext::Managed(handle) = actor else {
            return Ok(());
        };
        if self.sessions.get(handle.client()) == Some(&handle.id()) {
            Ok(())
        } else {
            Err(CoreError::SessionExpired {
                session: handle.id(),
            })
        }
    }

    fn install_node_changes(
        &mut self,
        candidate: BTreeMap<TopicPath, Node>,
        changes: Vec<Change>,
    ) -> Result<Option<UpdateBatch>, CoreError> {
        if changes.is_empty() {
            return Ok(None);
        }
        let next = self
            .sequence
            .0
            .checked_add(1)
            .map(CommitSequence)
            .ok_or(CoreError::SequenceExhausted)?;
        self.nodes = candidate;
        self.sequence = next;
        let update = UpdateBatch {
            sequence: next,
            changes,
        };
        self.publish_update(&update);
        Ok(Some(update))
    }

    fn publish_update(&mut self, update: &UpdateBatch) {
        let mut ended = Vec::new();
        for (id, subscriber) in &self.subscribers {
            if subscriber.updates.is_closed() {
                ended.push(*id);
                continue;
            }
            let changes = update
                .changes
                .iter()
                .filter(|change| subscriber.selection.matches(change.topic()))
                .cloned()
                .collect::<Vec<_>>();
            if changes.is_empty() {
                continue;
            }
            let projected = UpdateBatch {
                sequence: update.sequence,
                changes,
            };
            match subscriber.updates.try_send(projected) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    *subscriber
                        .end
                        .lock()
                        .expect("subscription end lock is not poisoned") =
                        Some(SubscriptionEnd::SlowConsumer);
                    ended.push(*id);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => ended.push(*id),
            }
        }
        for id in ended {
            self.subscribers.remove(&id);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitSequence(u64);

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
pub struct SubscriptionId(u64);

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

    const fn get(self) -> usize {
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
struct SubscriberState {
    selection: Selection,
    updates: mpsc::Sender<UpdateBatch>,
    end: Arc<Mutex<Option<SubscriptionEnd>>>,
}

#[derive(Debug)]
pub struct Subscription {
    id: SubscriptionId,
    snapshot: Snapshot,
    updates: mpsc::Receiver<UpdateBatch>,
    end: Arc<Mutex<Option<SubscriptionEnd>>>,
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
    sequence: CommitSequence,
    nodes: BTreeMap<TopicPath, Node>,
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
    update: UpdateBatch,
    warnings: Vec<Diagnostic>,
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
    sequence: CommitSequence,
    changes: Vec<Change>,
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
    fn topic(&self) -> &TopicPath {
        match self {
            Self::Upsert { topic, .. }
            | Self::Occurrence { topic, .. }
            | Self::Command { topic, .. }
            | Self::Removed { topic, .. } => topic,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenSessionOutcome {
    handle: SessionHandle,
    warnings: Vec<Diagnostic>,
    update: Option<UpdateBatch>,
    pending_releases: Vec<PendingClaimRelease>,
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
    was_current: bool,
    warnings: Vec<Diagnostic>,
    update: Option<UpdateBatch>,
    pending_releases: Vec<PendingClaimRelease>,
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
    topic: TopicPath,
    claim: ClaimId,
    deadline: Deadline,
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
}

fn validate_topics(batch: &WriteBatch) -> Result<(), CoreError> {
    for operation in batch.operations() {
        let topic = operation.topic();
        if topic.is_system() {
            return Err(CoreError::SystemTopic {
                topic: topic.clone(),
            });
        }
    }
    Ok(())
}

fn coalesce_batch_changes(
    previous: &BTreeMap<TopicPath, Node>,
    candidate: &BTreeMap<TopicPath, Node>,
    staged: Vec<Change>,
) -> Vec<Change> {
    let mut emitted_node_topics = BTreeSet::new();
    let mut changes = Vec::with_capacity(staged.len());
    for change in staged {
        match change {
            Change::Occurrence { .. } | Change::Command { .. } => changes.push(change),
            Change::Upsert { topic, .. } | Change::Removed { topic, .. } => {
                if emitted_node_topics.insert(topic.clone())
                    && let Some(change) = net_node_change(&topic, previous, candidate)
                {
                    changes.push(change);
                }
            }
        }
    }
    changes
}

fn net_node_change(
    topic: &TopicPath,
    previous: &BTreeMap<TopicPath, Node>,
    candidate: &BTreeMap<TopicPath, Node>,
) -> Option<Change> {
    match (previous.get(topic), candidate.get(topic)) {
        (Some(old), Some(new)) if old != new => Some(Change::Upsert {
            topic: topic.clone(),
            node: new.clone(),
        }),
        (None, Some(new)) => Some(Change::Upsert {
            topic: topic.clone(),
            node: new.clone(),
        }),
        (Some(old), None) => Some(Change::Removed {
            topic: topic.clone(),
            previous: old.clone(),
        }),
        _ => None,
    }
}

fn apply_operation(
    candidate: &mut BTreeMap<TopicPath, Node>,
    changes: &mut Vec<Change>,
    warnings: &mut Vec<Diagnostic>,
    actor: &WriteContext,
    operation: &WriteOperation,
    now: Timestamp,
    next_claim_id: &mut u64,
) -> Result<(), CoreError> {
    match operation {
        WriteOperation::PublishState {
            topic,
            value,
            expiry,
        } => {
            warn_on_output_replacement(
                candidate.get(topic),
                topic,
                NodeKind::State,
                actor.client(),
                warnings,
            );
            let expires_at = resolve_expiry(candidate.get(topic), topic, *expiry, now)?;
            let retained = RetainedValue::new(
                value.clone(),
                WriteProvenance::from_context(actor, now),
                expires_at,
            );
            let node = Node::State(StateNode::new(retained));
            candidate.insert(topic.clone(), node.clone());
            changes.push(Change::Upsert {
                topic: topic.clone(),
                node,
            });
        }
        WriteOperation::PublishEvent { topic, value } => {
            warn_on_output_replacement(
                candidate.get(topic),
                topic,
                NodeKind::Event,
                actor.client(),
                warnings,
            );
            let provenance = WriteProvenance::from_context(actor, now);
            let node = Node::Event(EventNode::new(provenance.clone()));
            let event = EventOccurrence::new(value.clone(), provenance);
            candidate.insert(topic.clone(), node.clone());
            changes.push(Change::Upsert {
                topic: topic.clone(),
                node,
            });
            changes.push(Change::Occurrence {
                topic: topic.clone(),
                event,
            });
        }
        WriteOperation::RemoveNode { topic } => {
            if let Some(previous) = candidate.remove(topic) {
                changes.push(Change::Removed {
                    topic: topic.clone(),
                    previous,
                });
            } else {
                warnings.push(Diagnostic::NodeAlreadyAbsent {
                    topic: topic.clone(),
                });
            }
        }
        WriteOperation::DefineInput {
            topic,
            kind,
            definition,
        } => {
            let previous = candidate.get(topic);
            if let Some(claim) = input_claim(previous)
                && actor.session_id() != Some(claim.session())
            {
                return Err(CoreError::ClaimAuthorityRequired {
                    topic: topic.clone(),
                });
            }
            warn_on_kind_change(previous, topic, node_kind(*kind), warnings);
            let node = match (kind, previous) {
                (InputKind::Desired, Some(Node::Desired(previous))) => {
                    Node::Desired(DesiredNode::from_parts(
                        definition.clone(),
                        previous.claim().cloned(),
                        previous.current().cloned(),
                    ))
                }
                (InputKind::Command, Some(Node::Command(previous))) => Node::Command(
                    CommandNode::from_parts(definition.clone(), previous.claim().cloned()),
                ),
                (InputKind::Desired, _) => Node::Desired(DesiredNode::new(definition.clone())),
                (InputKind::Command, _) => Node::Command(CommandNode::new(definition.clone())),
            };
            candidate.insert(topic.clone(), node.clone());
            changes.push(Change::Upsert {
                topic: topic.clone(),
                node,
            });
        }
        WriteOperation::ClaimInput { topic, release } => {
            let WriteContext::Managed(handle) = actor else {
                return Err(CoreError::SessionRequired {
                    operation: "claim_input",
                });
            };
            let existing =
                candidate
                    .get(topic)
                    .cloned()
                    .ok_or_else(|| CoreError::InputNotDefined {
                        topic: topic.clone(),
                    })?;
            if let Some(previous) = input_claim(Some(&existing))
                && previous.session() != handle.id()
            {
                warnings.push(Diagnostic::InputClaimReplaced {
                    topic: topic.clone(),
                    previous: previous.owner().clone(),
                    replacement: handle.client().clone(),
                });
            }
            let raw_claim = next_claim_id
                .checked_add(1)
                .ok_or(CoreError::ClaimIdExhausted)?;
            *next_claim_id = raw_claim;
            let claim = InputClaim::new(
                ClaimId::new(raw_claim),
                handle.client().clone(),
                handle.id(),
                *release,
            );
            let node = match existing {
                Node::Desired(previous) => Node::Desired(DesiredNode::from_parts(
                    previous.definition().clone(),
                    Some(claim),
                    previous.current().cloned(),
                )),
                Node::Command(previous) => Node::Command(CommandNode::from_parts(
                    previous.definition().clone(),
                    Some(claim),
                )),
                other => {
                    return Err(CoreError::NotAnInput {
                        topic: topic.clone(),
                        actual: other.kind(),
                    });
                }
            };
            candidate.insert(topic.clone(), node.clone());
            changes.push(Change::Upsert {
                topic: topic.clone(),
                node,
            });
        }
        WriteOperation::SubmitDesired {
            topic,
            value,
            expiry,
        } => {
            let previous =
                candidate
                    .get(topic)
                    .cloned()
                    .ok_or_else(|| CoreError::InputNotDefined {
                        topic: topic.clone(),
                    })?;
            let Node::Desired(previous) = previous else {
                return Err(CoreError::WrongInputKind {
                    topic: topic.clone(),
                    expected: InputKind::Desired,
                    actual: previous.kind(),
                });
            };
            let expires_at = resolve_expiry(candidate.get(topic), topic, *expiry, now)?;
            let current = RetainedValue::new(
                value.clone(),
                WriteProvenance::from_context(actor, now),
                expires_at,
            );
            let node = Node::Desired(DesiredNode::from_parts(
                previous.definition().clone(),
                previous.claim().cloned(),
                Some(current),
            ));
            candidate.insert(topic.clone(), node.clone());
            changes.push(Change::Upsert {
                topic: topic.clone(),
                node,
            });
        }
        WriteOperation::SubmitCommand { topic, value } => {
            let previous = candidate
                .get(topic)
                .ok_or_else(|| CoreError::InputNotDefined {
                    topic: topic.clone(),
                })?;
            if !matches!(previous, Node::Command(_)) {
                return Err(CoreError::WrongInputKind {
                    topic: topic.clone(),
                    expected: InputKind::Command,
                    actual: previous.kind(),
                });
            }
            changes.push(Change::Command {
                topic: topic.clone(),
                command: CommandOccurrence::new(
                    value.clone(),
                    WriteProvenance::from_context(actor, now),
                ),
            });
        }
        WriteOperation::ClearDesired { topic } => {
            let previous =
                candidate
                    .get(topic)
                    .cloned()
                    .ok_or_else(|| CoreError::InputNotDefined {
                        topic: topic.clone(),
                    })?;
            let Node::Desired(previous) = previous else {
                return Err(CoreError::WrongInputKind {
                    topic: topic.clone(),
                    expected: InputKind::Desired,
                    actual: previous.kind(),
                });
            };
            let node = Node::Desired(DesiredNode::from_parts(
                previous.definition().clone(),
                previous.claim().cloned(),
                None,
            ));
            candidate.insert(topic.clone(), node.clone());
            changes.push(Change::Upsert {
                topic: topic.clone(),
                node,
            });
        }
    }
    Ok(())
}

fn warn_on_output_replacement(
    previous: Option<&Node>,
    topic: &TopicPath,
    replacement_kind: NodeKind,
    replacement_owner: &ClientName,
    warnings: &mut Vec<Diagnostic>,
) {
    let Some(previous) = previous else {
        return;
    };

    if previous.kind() != replacement_kind {
        warnings.push(Diagnostic::NodeKindChanged {
            topic: topic.clone(),
            previous: previous.kind(),
            replacement: replacement_kind,
        });
    }

    let previous_owner = match previous {
        Node::State(state) => Some(state.current().last_write().client()),
        Node::Event(event) => Some(event.last_publisher().client()),
        Node::Desired(_) | Node::Command(_) => None,
    };
    if let Some(previous_owner) = previous_owner.filter(|owner| *owner != replacement_owner) {
        warnings.push(Diagnostic::OutputOwnerChanged {
            topic: topic.clone(),
            previous: previous_owner.clone(),
            replacement: replacement_owner.clone(),
        });
    }
}

fn warn_on_kind_change(
    previous: Option<&Node>,
    topic: &TopicPath,
    replacement: NodeKind,
    warnings: &mut Vec<Diagnostic>,
) {
    if let Some(previous) = previous.filter(|previous| previous.kind() != replacement) {
        warnings.push(Diagnostic::NodeKindChanged {
            topic: topic.clone(),
            previous: previous.kind(),
            replacement,
        });
    }
}

const fn node_kind(kind: InputKind) -> NodeKind {
    match kind {
        InputKind::Desired => NodeKind::Desired,
        InputKind::Command => NodeKind::Command,
    }
}

fn input_claim(node: Option<&Node>) -> Option<&InputClaim> {
    match node {
        Some(Node::Desired(node)) => node.claim(),
        Some(Node::Command(node)) => node.claim(),
        Some(Node::State(_) | Node::Event(_)) | None => None,
    }
}

fn resolve_expiry(
    previous: Option<&Node>,
    topic: &TopicPath,
    update: ExpiryUpdate,
    now: Timestamp,
) -> Result<Option<Deadline>, CoreError> {
    match update {
        ExpiryUpdate::Preserve => Ok(match previous {
            Some(Node::State(state)) => state.current().expires_at(),
            Some(Node::Desired(desired)) => desired.current().and_then(RetainedValue::expires_at),
            _ => None,
        }),
        ExpiryUpdate::Clear => Ok(None),
        ExpiryUpdate::Set(duration) => now
            .get()
            .checked_add(duration.get())
            .map(Timestamp::new)
            .map(Deadline::new)
            .map(Some)
            .map_err(|source| CoreError::DeadlineOutOfRange {
                topic: topic.clone(),
                source,
            }),
    }
}

fn collect_session_releases(
    nodes: &mut BTreeMap<TopicPath, Node>,
    session: SessionId,
    now: Timestamp,
    changes: &mut Vec<Change>,
    warnings: &mut Vec<Diagnostic>,
    pending: &mut Vec<PendingClaimRelease>,
) {
    for (topic, node) in nodes.iter_mut() {
        let Some(claim) = input_claim(Some(node)).filter(|claim| claim.session() == session) else {
            continue;
        };
        let claim_id = claim.id();
        let release = claim.release();
        match release {
            ClaimRelease::Immediate => {
                clear_claim(node);
                changes.push(Change::Upsert {
                    topic: topic.clone(),
                    node: node.clone(),
                });
            }
            ClaimRelease::After(duration) => match now.get().checked_add(duration.get()) {
                Ok(deadline) => pending.push(PendingClaimRelease {
                    topic: topic.clone(),
                    claim: claim_id,
                    deadline: Deadline::new(Timestamp::new(deadline)),
                }),
                Err(_) => {
                    warnings.push(Diagnostic::ClaimGraceOutOfRange {
                        topic: topic.clone(),
                    });
                    clear_claim(node);
                    changes.push(Change::Upsert {
                        topic: topic.clone(),
                        node: node.clone(),
                    });
                }
            },
        }
    }
}

fn clear_claim(node: &mut Node) {
    match node {
        Node::Desired(previous) => {
            *previous = DesiredNode::from_parts(
                previous.definition().clone(),
                None,
                previous.current().cloned(),
            );
        }
        Node::Command(previous) => {
            *previous = CommandNode::from_parts(previous.definition().clone(), None);
        }
        Node::State(_) | Node::Event(_) => unreachable!("only input nodes carry claims"),
    }
}
