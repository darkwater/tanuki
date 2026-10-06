use std::collections::BTreeMap;

use thiserror::Error;

use crate::wire::{ChangeView, NodeView, SnapshotView, UpdateView};

#[derive(Clone, Debug, PartialEq)]
pub struct SelectedView {
    sequence: u64,
    nodes: BTreeMap<String, NodeView>,
}

impl SelectedView {
    #[must_use]
    pub fn from_snapshot(snapshot: SnapshotView) -> Self {
        Self {
            sequence: snapshot.sequence,
            nodes: snapshot.nodes,
        }
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<String, NodeView> {
        &self.nodes
    }

    pub fn apply(&mut self, update: UpdateView) -> Result<AppliedUpdate, ClientViewError> {
        if update.sequence <= self.sequence {
            return Err(ClientViewError::OutOfOrder {
                current: self.sequence,
                received: update.sequence,
            });
        }
        let mut candidate = self.nodes.clone();
        let mut occurrences = Vec::new();
        for change in update.changes {
            match change {
                ChangeView::Upsert { topic, node } => {
                    candidate.insert(topic, node);
                }
                ChangeView::Removed { topic, .. } => {
                    candidate.remove(&topic);
                }
                occurrence @ (ChangeView::Event { .. } | ChangeView::Command { .. }) => {
                    occurrences.push(occurrence);
                }
            }
        }
        self.nodes = candidate;
        self.sequence = update.sequence;
        Ok(AppliedUpdate { occurrences })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedUpdate {
    occurrences: Vec<ChangeView>,
}

impl AppliedUpdate {
    #[must_use]
    pub fn occurrences(&self) -> &[ChangeView] {
        &self.occurrences
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ClientViewError {
    #[error("received commit sequence {received} after {current}")]
    OutOfOrder { current: u64, received: u64 },
}
