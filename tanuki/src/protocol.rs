//! Server-side conversion from authoritative domain state to shared wire DTOs.
use crate::{
    core::{Change, Diagnostic, Snapshot, UpdateBatch},
    domain::{
        ClaimRelease, CommandOccurrence, EventOccurrence, InputClaim, Node, RetainedValue,
        WriteProvenance,
    },
};
pub use tanuki_protocol::*;

impl From<&Snapshot> for SnapshotView {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            sequence: snapshot.sequence().get(),
            nodes: snapshot
                .nodes()
                .iter()
                .map(|(topic, node)| (topic.to_string(), NodeView::from(node)))
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
                definition: InputDefinitionView {},
                claim: node.claim().map(ClaimView::from),
                current: node.current().map(CurrentValueView::from),
            },
            Node::Command(node) => Self::Command {
                definition: InputDefinitionView {},
                claim: node.claim().map(ClaimView::from),
            },
        }
    }
}

impl From<&RetainedValue> for CurrentValueView {
    fn from(value: &RetainedValue) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
            last_write: ProvenanceView::from(value.last_write()),
            expires_at: value
                .expires_at()
                .map(|deadline| deadline.get().get().to_string()),
        }
    }
}

impl From<&WriteProvenance> for ProvenanceView {
    fn from(value: &WriteProvenance) -> Self {
        Self {
            client: value.client().as_str().to_owned(),
            session: value.session().map(|session| session.get()),
            at: value.at().get().to_string(),
        }
    }
}

impl From<&InputClaim> for ClaimView {
    fn from(value: &InputClaim) -> Self {
        Self {
            id: value.id().get(),
            owner: value.owner().as_str().to_owned(),
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
                duration: duration_string(duration),
            },
        }
    }
}

impl From<&Change> for ChangeView {
    fn from(change: &Change) -> Self {
        match change {
            Change::Upsert { topic, node } => Self::Upsert {
                topic: topic.to_string(),
                node: NodeView::from(node),
            },
            Change::Occurrence { topic, event } => Self::Event {
                topic: topic.to_string(),
                event: OccurrenceView::from(event),
            },
            Change::Command { topic, command } => Self::Command {
                topic: topic.to_string(),
                command: OccurrenceView::from(command),
            },
            Change::Removed { topic, previous } => Self::Removed {
                topic: topic.to_string(),
                previous: NodeView::from(previous),
            },
        }
    }
}

impl From<&EventOccurrence> for OccurrenceView {
    fn from(value: &EventOccurrence) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
            provenance: ProvenanceView::from(value.publisher()),
        }
    }
}

impl From<&CommandOccurrence> for OccurrenceView {
    fn from(value: &CommandOccurrence) -> Self {
        Self {
            value: JsonValue::new(value.value().clone()),
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
                topic: topic.to_string(),
                previous: previous.as_str().to_owned(),
                replacement: replacement.as_str().to_owned(),
            },
            Diagnostic::NodeKindChanged {
                topic,
                previous,
                replacement,
            } => Self::NodeKindChanged {
                topic: topic.to_string(),
                previous: format!("{previous:?}").to_lowercase(),
                replacement: format!("{replacement:?}").to_lowercase(),
            },
            Diagnostic::NodeAlreadyAbsent { topic } => Self::NodeAlreadyAbsent {
                topic: topic.to_string(),
            },
            Diagnostic::SessionReplaced {
                client,
                previous,
                replacement,
            } => Self::SessionReplaced {
                client: client.as_str().to_owned(),
                previous: previous.get(),
                replacement: replacement.get(),
            },
            Diagnostic::InputClaimReplaced {
                topic,
                previous,
                replacement,
            } => Self::InputClaimReplaced {
                topic: topic.to_string(),
                previous: previous.as_str().to_owned(),
                replacement: replacement.as_str().to_owned(),
            },
            Diagnostic::ClaimGraceOutOfRange { topic } => Self::ClaimGraceOutOfRange {
                topic: topic.to_string(),
            },
            Diagnostic::SchemaWarning(issue) => Self::SchemaWarning {
                topic: issue.topic().to_string(),
                message: issue.kind().to_string(),
            },
            Diagnostic::LinkDisabled { link, issue } => Self::LinkDisabled {
                link: link.as_str().to_owned(),
                topic: issue.topic().to_string(),
                message: issue.kind().to_string(),
            },
            Diagnostic::LinkEnabled { link } => Self::LinkEnabled {
                link: link.as_str().to_owned(),
            },
        }
    }
}
fn duration_string(value: NonNegativeDuration) -> String {
    jiff::fmt::temporal::SpanPrinter::new().duration_to_string(&value.get())
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
