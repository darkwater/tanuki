//! Server-side conversion between authoritative domain types and wire DTOs.
mod request;
use crate::{
    core::{Change, Diagnostic, Snapshot, UpdateBatch},
    domain::{
        ClaimRelease, CommandOccurrence, EventOccurrence, InputClaim, Node, RetainedValue,
        WriteProvenance,
    },
};
pub use request::{RequestConversionError, expiry_update, nonnegative_duration};
use serde::Serialize;
pub use tanuki_protocol::*;

impl From<&crate::core::CommitOutcome> for CommitView {
    fn from(outcome: &crate::core::CommitOutcome) -> Self {
        Self {
            sequence: outcome.update().sequence().get(),
            warnings: outcome
                .warnings()
                .iter()
                .map(DiagnosticView::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct LinkInstallView {
    link: String,
    enabled: bool,
    sequence: Option<u64>,
    warnings: Vec<DiagnosticView>,
}

impl LinkInstallView {
    pub fn new(name: &crate::link::LinkName, outcome: &crate::core::LinkInstallOutcome) -> Self {
        Self {
            link: name.to_string(),
            enabled: outcome.enabled(),
            sequence: outcome.update().map(|update| update.sequence().get()),
            warnings: outcome
                .warnings()
                .iter()
                .map(DiagnosticView::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct LinkRemovalView {
    link: String,
    removed: bool,
    sequence: Option<u64>,
}

impl LinkRemovalView {
    pub fn new(name: &crate::link::LinkName, outcome: &crate::core::LinkRemovalOutcome) -> Self {
        Self {
            link: name.to_string(),
            removed: outcome.removed(),
            sequence: outcome.update().map(|update| update.sequence().get()),
        }
    }
}

#[derive(Serialize)]
pub struct SchemaInstallView {
    schema: String,
    sequence: Option<u64>,
    warnings: Vec<DiagnosticView>,
}

impl SchemaInstallView {
    pub fn new(name: String, outcome: &crate::core::SchemaInstallOutcome) -> Self {
        Self {
            schema: name,
            sequence: outcome.update().map(|update| update.sequence().get()),
            warnings: outcome
                .warnings()
                .iter()
                .map(DiagnosticView::from)
                .collect(),
        }
    }
}

impl From<&Snapshot> for SnapshotView {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            sequence: snapshot.sequence().get(),
            nodes: snapshot
                .nodes()
                .iter()
                .map(|(topic, node)| (topic.clone(), NodeView::from(node)))
                .collect(),
        }
    }
}

impl From<&UpdateBatch> for UpdateView {
    fn from(update: &UpdateBatch) -> Self {
        Self {
            sequence: update.sequence().get(),
            changes: update.changes().iter().map(ChangeView::from).collect(),
        }
    }
}

impl From<&Node> for NodeView {
    fn from(node: &Node) -> Self {
        match node {
            Node::State(node) => Self::State {
                current: CurrentValueView::from(node.current()),
            },
            Node::Event(node) => Self::Event {
                last_publisher: ProvenanceView::from(node.last_publisher()),
            },
            Node::Desired(node) => Self::Desired {
                claim: node.claim().map(ClaimView::from),
                current: node.current().map(CurrentValueView::from),
            },
            Node::Command(node) => Self::Command {
                claim: node.claim().map(ClaimView::from),
            },
        }
    }
}

impl From<&RetainedValue> for CurrentValueView {
    fn from(value: &RetainedValue) -> Self {
        Self {
            value: WireValue::new(value.value().clone()),
            last_write: ProvenanceView::from(value.last_write()),
            expires_at: value.expires_at().map(|deadline| deadline.get().get()),
        }
    }
}

impl From<&WriteProvenance> for ProvenanceView {
    fn from(value: &WriteProvenance) -> Self {
        Self {
            client: value.client().clone(),
            session: value.session().map(|session| session.get()),
            at: value.at().get(),
        }
    }
}

impl From<&InputClaim> for ClaimView {
    fn from(value: &InputClaim) -> Self {
        Self {
            id: value.id().get(),
            owner: value.owner().clone(),
            session: value.session().get(),
            release: ClaimReleaseView::from(value.release()),
        }
    }
}

impl From<ClaimRelease> for ClaimReleaseView {
    fn from(value: ClaimRelease) -> Self {
        match value {
            ClaimRelease::Immediate => Self::Immediate,
            ClaimRelease::After(duration) => Self::After {
                duration: duration.get(),
            },
        }
    }
}

impl From<&Change> for ChangeView {
    fn from(change: &Change) -> Self {
        match change {
            Change::Upsert { topic, node } => Self::Upsert {
                topic: topic.clone(),
                node: NodeView::from(node),
            },
            Change::Occurrence { topic, event } => Self::Event {
                topic: topic.clone(),
                event: OccurrenceView::from(event),
            },
            Change::Command { topic, command } => Self::Command {
                topic: topic.clone(),
                command: OccurrenceView::from(command),
            },
            Change::Removed { topic, previous } => Self::Removed {
                topic: topic.clone(),
                previous: NodeView::from(previous),
            },
        }
    }
}

impl From<&EventOccurrence> for OccurrenceView {
    fn from(value: &EventOccurrence) -> Self {
        Self {
            value: WireValue::new(value.value().clone()),
            provenance: ProvenanceView::from(value.publisher()),
        }
    }
}

impl From<&CommandOccurrence> for OccurrenceView {
    fn from(value: &CommandOccurrence) -> Self {
        Self {
            value: WireValue::new(value.value().clone()),
            provenance: ProvenanceView::from(value.submitter()),
        }
    }
}

impl From<&Diagnostic> for DiagnosticView {
    fn from(value: &Diagnostic) -> Self {
        match value {
            Diagnostic::OutputOwnerChanged {
                topic,
                previous,
                replacement,
            } => Self::OutputOwnerChanged {
                topic: topic.clone(),
                previous: previous.clone(),
                replacement: replacement.clone(),
            },
            Diagnostic::NodeKindChanged {
                topic,
                previous,
                replacement,
            } => Self::NodeKindChanged {
                topic: topic.clone(),
                previous: node_kind(*previous),
                replacement: node_kind(*replacement),
            },
            Diagnostic::NodeAlreadyAbsent { topic } => Self::NodeAlreadyAbsent {
                topic: topic.clone(),
            },
            Diagnostic::SessionReplaced {
                client,
                previous,
                replacement,
            } => Self::SessionReplaced {
                client: client.clone(),
                previous: previous.get(),
                replacement: replacement.get(),
            },
            Diagnostic::InputClaimReplaced {
                topic,
                previous,
                replacement,
            } => Self::InputClaimReplaced {
                topic: topic.clone(),
                previous: previous.clone(),
                replacement: replacement.clone(),
            },
            Diagnostic::ClaimGraceOutOfRange { topic } => Self::ClaimGraceOutOfRange {
                topic: topic.clone(),
            },
            Diagnostic::SchemaWarning(issue) => Self::SchemaWarning {
                topic: issue.topic().clone(),
                message: issue.kind().to_string(),
            },
            Diagnostic::LinkDisabled { link, issue } => Self::LinkDisabled {
                link: link.as_str().to_owned(),
                topic: issue.topic().clone(),
                message: issue.kind().to_string(),
            },
            Diagnostic::LinkEnabled { link } => Self::LinkEnabled {
                link: link.as_str().to_owned(),
            },
        }
    }
}
const fn node_kind(kind: crate::domain::NodeKind) -> NodeKind {
    match kind {
        crate::domain::NodeKind::State => NodeKind::State,
        crate::domain::NodeKind::Event => NodeKind::Event,
        crate::domain::NodeKind::Desired => NodeKind::Desired,
        crate::domain::NodeKind::Command => NodeKind::Command,
    }
}

#[must_use]
pub fn snapshot_message(
    request_id: RequestId,
    snapshot: &Snapshot,
    warnings: &[Diagnostic],
) -> ServerMessage {
    let view = SnapshotView::from(snapshot);
    ServerMessage::Snapshot {
        request_id,
        sequence: view.sequence,
        nodes: view.nodes,
        warnings: warnings.iter().map(DiagnosticView::from).collect(),
    }
}

#[must_use]
pub fn update_message(update: &UpdateBatch) -> ServerMessage {
    let view = UpdateView::from(update);
    ServerMessage::Update {
        sequence: view.sequence,
        changes: view.changes,
    }
}
