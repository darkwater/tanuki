use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::domain::{
    ClientName, Deadline, EventNode, EventOccurrence, ExpiryUpdate, Node, NodeKind, RetainedValue,
    Selection, StateNode, Timestamp, TopicPath, WriteBatch, WriteContext, WriteOperation,
    WriteProvenance,
};

#[derive(Debug, Default)]
pub struct Core {
    sequence: CommitSequence,
    nodes: BTreeMap<TopicPath, Node>,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
        validate_targets(&batch)?;

        let next_sequence = self
            .sequence
            .0
            .checked_add(1)
            .map(CommitSequence)
            .ok_or(CoreError::SequenceExhausted)?;
        let mut candidate = self.nodes.clone();
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
            )?;
        }

        debug_assert!(candidate.keys().all(|topic| !topic.is_system()));
        self.nodes = candidate;
        self.sequence = next_sequence;
        Ok(CommitOutcome {
            update: UpdateBatch {
                sequence: next_sequence,
                changes,
            },
            warnings,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitSequence(u64);

impl CommitSequence {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
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
    Removed {
        topic: TopicPath,
        previous: Node,
    },
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
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("writes to reserved system topic `{topic}` are not permitted")]
    SystemTopic { topic: TopicPath },
    #[error("write batch targets `{topic}` more than once")]
    DuplicateTarget { topic: TopicPath },
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
}

fn validate_targets(batch: &WriteBatch) -> Result<(), CoreError> {
    let mut targets = BTreeSet::new();
    for operation in batch.operations() {
        let topic = operation.topic();
        if topic.is_system() {
            return Err(CoreError::SystemTopic {
                topic: topic.clone(),
            });
        }
        if !targets.insert(topic.clone()) {
            return Err(CoreError::DuplicateTarget {
                topic: topic.clone(),
            });
        }
    }
    Ok(())
}

fn apply_operation(
    candidate: &mut BTreeMap<TopicPath, Node>,
    changes: &mut Vec<Change>,
    warnings: &mut Vec<Diagnostic>,
    actor: &WriteContext,
    operation: &WriteOperation,
    now: Timestamp,
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
        WriteOperation::DefineInput { .. } => {
            return Err(CoreError::UnsupportedOperation {
                operation: "define_input",
            });
        }
        WriteOperation::ClaimInput { .. } => {
            return Err(CoreError::UnsupportedOperation {
                operation: "claim_input",
            });
        }
        WriteOperation::SubmitDesired { .. } => {
            return Err(CoreError::UnsupportedOperation {
                operation: "submit_desired",
            });
        }
        WriteOperation::SubmitCommand { .. } => {
            return Err(CoreError::UnsupportedOperation {
                operation: "submit_command",
            });
        }
        WriteOperation::ClearDesired { .. } => {
            return Err(CoreError::UnsupportedOperation {
                operation: "clear_desired",
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

fn resolve_expiry(
    previous: Option<&Node>,
    topic: &TopicPath,
    update: ExpiryUpdate,
    now: Timestamp,
) -> Result<Option<Deadline>, CoreError> {
    match update {
        ExpiryUpdate::Preserve => Ok(match previous {
            Some(Node::State(state)) => state.current().expires_at(),
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
