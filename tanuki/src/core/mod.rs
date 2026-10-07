//! Authoritative state and atomic commit coordination.
mod diagnostics;
mod mutation;
mod sessions;
mod subscriptions;
mod types;
mod views;

use diagnostics::*;
use mutation::*;
use sessions::*;
pub use types::*;
use views::*;

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use thiserror::Error;
use tokio::sync::mpsc;
use tracing::warn;

use crate::domain::{
    ClaimId, ClaimRelease, ClientName, CommandNode, CommandOccurrence, Deadline, DesiredNode,
    EventNode, EventOccurrence, ExpiryUpdate, InputClaim, InputKind, Node, NodeKind, RetainedValue,
    Selection, SessionHandle, SessionId, StateNode, Timestamp, TopicPath, Value, WriteBatch,
    WriteContext, WriteOperation, WriteProvenance,
};
use crate::link::{LinkDefinition, LinkInstallError, LinkName, LinkRegistry};
use crate::schema::{
    Enforcement, Schema, SchemaIssue, SchemaRegistry, SchemaRegistryError, SchemaViolation,
};

#[derive(Clone, Copy)]
enum CommitPolicy {
    Always,
    IfChanged,
}

#[derive(Debug, Default)]
pub struct Core {
    sequence: CommitSequence,
    nodes: BTreeMap<TopicPath, Node>,
    sessions: BTreeMap<ClientName, SessionId>,
    next_session_id: u64,
    next_claim_id: u64,
    subscribers: BTreeMap<SubscriptionId, SubscriberState>,
    next_subscription_id: u64,
    schemas: SchemaRegistry,
    links: LinkRegistry,
    diagnostics: BTreeMap<TopicPath, Node>,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_restored(
        sequence: CommitSequence,
        nodes: BTreeMap<TopicPath, Node>,
        schemas: SchemaRegistry,
        links: Vec<LinkDefinition>,
        now: Timestamp,
    ) -> Result<Self, RestoreError> {
        let mut warnings = Vec::new();
        for (topic, node) in &nodes {
            if topic.is_system() {
                return Err(RestoreError::SystemTopic {
                    topic: topic.clone(),
                });
            }
            for (enforcement, issue) in
                schemas.inspect_existing(topic, node.kind(), node_value(node))
            {
                match enforcement {
                    Enforcement::Deny => return Err(RestoreError::SchemaViolation { issue }),
                    Enforcement::Warn => warnings.push(issue),
                }
            }
        }
        let mut core = Self {
            sequence,
            nodes,
            schemas,
            ..Self::default()
        };
        for definition in links {
            if let Some(topic) = core
                .nodes
                .keys()
                .find(|topic| definition.mount().is_prefix_of(topic))
            {
                return Err(LinkInstallError::DestinationCollision {
                    mount: definition.mount().clone(),
                    topic: topic.clone(),
                }
                .into());
            }
            let (enabled, issues, denial) =
                validate_link_retained(&definition, &core.nodes, &core.schemas);
            warnings.extend(issues);
            debug_assert_eq!(enabled, denial.is_none());
            core.links.install(definition, denial)?;
        }
        core.diagnostics = active_diagnostics(
            &core.nodes,
            &core.links,
            &core.schemas,
            &BTreeMap::new(),
            now,
        );
        for issue in warnings {
            warn!(?issue, "restored value accepted with schema warning");
        }
        Ok(core)
    }

    /// Capture canonical state and its policy together, excluding live sessions,
    /// subscribers, projected aliases, and derived diagnostics.
    #[must_use]
    pub fn persistence_snapshot(&self) -> PersistenceSnapshot {
        PersistenceSnapshot {
            sequence: self.sequence,
            nodes: self.nodes.clone(),
            schemas: self.schemas.clone(),
            links: self
                .links
                .iter()
                .map(|link| link.definition().clone())
                .collect(),
        }
    }

    #[must_use]
    pub fn read(&self, selection: &Selection) -> Snapshot {
        let mut visible = visible_nodes(&self.nodes, &self.links);
        visible.extend(self.diagnostics.clone());
        Snapshot {
            sequence: self.sequence,
            nodes: visible
                .iter()
                .filter(|(topic, _)| selection.matches(topic))
                .map(|(topic, node)| (topic.clone(), node.clone()))
                .collect(),
        }
    }

    #[must_use]
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    pub fn apply(
        &mut self,
        actor: &WriteContext,
        batch: WriteBatch,
        now: Timestamp,
    ) -> Result<CommitOutcome, CoreError> {
        self.validate_actor(actor)?;
        validate_topics(&batch)?;

        let previous = self.nodes.clone();
        let mut candidate = previous.clone();
        let mut candidate_links = self.links.clone();
        let mut next_claim_id = self.next_claim_id;
        let mut changes = Vec::new();
        let mut warnings = Vec::new();

        for operation in batch.operations() {
            let original_kind = operation_value_kind(operation);
            let alias_topic = self
                .links
                .resolve_alias(operation.topic())
                .map(|_| operation.topic().clone());
            let operation =
                prepare_alias_operation(&self.links, &self.schemas, operation, &mut warnings)?;
            let alias_kind = operation_value_kind(&operation);
            let operation = validate_operation(&self.schemas, &operation, &mut warnings)?;
            let canonical_kind = operation_value_kind(&operation);
            if let Some(alias_topic) = alias_topic
                && original_kind != alias_kind
                && alias_kind != canonical_kind
            {
                warn!(
                    topic = %alias_topic,
                    ?original_kind,
                    ?alias_kind,
                    ?canonical_kind,
                    "alias and canonical schemas both cast one write"
                );
            }
            apply_operation(
                &mut candidate,
                &mut changes,
                &mut warnings,
                actor,
                &operation,
                now,
                &mut next_claim_id,
            )?;
        }

        let changes = coalesce_batch_changes(&previous, &candidate, changes);
        let mut changes = reconcile_link_views(
            (&previous, &self.links),
            &candidate,
            &mut candidate_links,
            &self.schemas,
            changes,
            &mut warnings,
            false,
        );
        let candidate_diagnostics = active_diagnostics(
            &candidate,
            &candidate_links,
            &self.schemas,
            &self.diagnostics,
            now,
        );
        changes.extend(diff_visible_nodes(
            &self.diagnostics,
            &candidate_diagnostics,
        ));

        debug_assert!(candidate.keys().all(|topic| !topic.is_system()));
        debug_assert!(nodes_have_no_denying_schema_violations(
            &self.schemas,
            &candidate
        ));
        let update = self
            .commit(changes, CommitPolicy::Always, |core| {
                core.nodes = candidate;
                core.links = candidate_links;
                core.diagnostics = candidate_diagnostics;
                core.next_claim_id = next_claim_id;
            })?
            .expect("accepted writes always produce a commit");
        Ok(CommitOutcome { update, warnings })
    }

    pub fn install_schema(
        &mut self,
        schema: Schema,
        mode: SchemaInstallMode,
        now: Timestamp,
    ) -> Result<SchemaInstallOutcome, CoreError> {
        let mut candidate_registry = self.schemas.clone();
        candidate_registry.install(schema.clone())?;

        let mut warnings = Vec::new();
        let mut invalid_topics = BTreeSet::new();
        let mut structurally_invalid_topics = BTreeSet::new();
        let mut violations = Vec::new();
        for (topic, node) in &self.nodes {
            let value = match node {
                Node::State(state) => Some(state.current().value()),
                Node::Desired(desired) => desired.current().map(RetainedValue::value),
                Node::Event(_) | Node::Command(_) => None,
            };
            for (enforcement, issue) in schema.inspect_existing(topic, node.kind(), value) {
                match enforcement {
                    Enforcement::Warn => warnings.push(Diagnostic::SchemaWarning(issue)),
                    Enforcement::Deny => {
                        invalid_topics.insert(topic.clone());
                        if matches!(
                            issue.kind(),
                            crate::schema::ViolationKind::WrongNodeKind { .. }
                        ) {
                            structurally_invalid_topics.insert(topic.clone());
                        }
                        violations.push(issue);
                    }
                }
            }
        }

        if !violations.is_empty() && mode == SchemaInstallMode::RejectInvalid {
            return Err(CoreError::ExistingSchemaViolations { violations });
        }

        let mut candidate_nodes = self.nodes.clone();
        let mut changes = Vec::new();
        if mode == SchemaInstallMode::RemoveInvalid {
            for topic in invalid_topics {
                let Some(previous) = candidate_nodes.get(&topic).cloned() else {
                    continue;
                };
                match previous {
                    Node::State(_) => {
                        candidate_nodes.remove(&topic);
                    }
                    Node::Desired(desired) if !structurally_invalid_topics.contains(&topic) => {
                        candidate_nodes.insert(
                            topic.clone(),
                            Node::Desired(DesiredNode::from_parts(
                                desired.definition().clone(),
                                desired.claim().cloned(),
                                None,
                            )),
                        );
                    }
                    Node::Desired(_) | Node::Event(_) | Node::Command(_) => {
                        candidate_nodes.remove(&topic);
                    }
                }
                if let Some(change) = net_node_change(&topic, &self.nodes, &candidate_nodes) {
                    changes.push(change);
                }
            }
        }

        let mut candidate_links = self.links.clone();
        let mut changes = reconcile_link_views(
            (&self.nodes, &self.links),
            &candidate_nodes,
            &mut candidate_links,
            &candidate_registry,
            changes,
            &mut warnings,
            true,
        );
        let candidate_diagnostics = active_diagnostics(
            &candidate_nodes,
            &candidate_links,
            &candidate_registry,
            &self.diagnostics,
            now,
        );
        changes.extend(diff_visible_nodes(
            &self.diagnostics,
            &candidate_diagnostics,
        ));
        let update = self.commit(changes, CommitPolicy::IfChanged, |core| {
            core.nodes = candidate_nodes;
            core.schemas = candidate_registry;
            core.links = candidate_links;
            core.diagnostics = candidate_diagnostics;
        })?;
        Ok(SchemaInstallOutcome { warnings, update })
    }

    pub fn install_link(
        &mut self,
        definition: LinkDefinition,
        now: Timestamp,
    ) -> Result<LinkInstallOutcome, CoreError> {
        if let Some(topic) = self
            .nodes
            .keys()
            .find(|topic| definition.mount().is_prefix_of(topic))
        {
            return Err(LinkInstallError::DestinationCollision {
                mount: definition.mount().clone(),
                topic: topic.clone(),
            }
            .into());
        }

        let old_visible = visible_nodes(&self.nodes, &self.links);
        let (enabled, issues, denial) =
            validate_link_retained(&definition, &self.nodes, &self.schemas);
        let mut candidate_links = self.links.clone();
        candidate_links.install(definition.clone(), denial.clone())?;
        let warnings = issues
            .into_iter()
            .map(Diagnostic::SchemaWarning)
            .chain(denial.map(|issue| Diagnostic::LinkDisabled {
                link: definition.name().clone(),
                issue,
            }))
            .collect::<Vec<_>>();
        let new_visible = visible_nodes(&self.nodes, &candidate_links);
        let mut changes = diff_visible_nodes(&old_visible, &new_visible);
        let candidate_diagnostics = active_diagnostics(
            &self.nodes,
            &candidate_links,
            &self.schemas,
            &self.diagnostics,
            now,
        );
        changes.extend(diff_visible_nodes(
            &self.diagnostics,
            &candidate_diagnostics,
        ));
        let update = self.commit(changes, CommitPolicy::IfChanged, |core| {
            core.links = candidate_links;
            core.diagnostics = candidate_diagnostics;
        })?;
        Ok(LinkInstallOutcome {
            enabled,
            warnings,
            update,
        })
    }

    pub fn remove_link(
        &mut self,
        name: &LinkName,
        now: Timestamp,
    ) -> Result<LinkRemovalOutcome, CoreError> {
        let old_projected = projected_nodes(&self.nodes, &self.links);
        let mut candidate_links = self.links.clone();
        let removed = candidate_links.remove(name).is_some();
        if !removed {
            return Ok(LinkRemovalOutcome {
                removed: false,
                update: None,
            });
        }
        let new_projected = projected_nodes(&self.nodes, &candidate_links);
        let mut changes = diff_visible_nodes(&old_projected, &new_projected);
        let candidate_diagnostics = active_diagnostics(
            &self.nodes,
            &candidate_links,
            &self.schemas,
            &self.diagnostics,
            now,
        );
        changes.extend(diff_visible_nodes(
            &self.diagnostics,
            &candidate_diagnostics,
        ));
        let update = self.commit(changes, CommitPolicy::IfChanged, |core| {
            core.links = candidate_links;
            core.diagnostics = candidate_diagnostics;
        })?;
        Ok(LinkRemovalOutcome {
            removed: true,
            update,
        })
    }

    #[must_use]
    pub fn next_value_deadline(&self) -> Option<Deadline> {
        let expiry = self
            .nodes
            .values()
            .filter_map(Node::retained_value)
            .filter_map(RetainedValue::expires_at)
            .min();
        let freshness = visible_nodes(&self.nodes, &self.links)
            .iter()
            .filter(|(topic, _)| {
                !self
                    .diagnostics
                    .contains_key(&freshness_diagnostic_topic(topic))
            })
            .filter_map(|(topic, node)| freshness_deadline(topic, node, &self.schemas))
            .min();
        [expiry, freshness].into_iter().flatten().min()
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
        let (update, warnings) = self.install_node_changes(candidate, changes, now)?;
        for warning in warnings {
            warn!(?warning, "deadline mutation produced diagnostic");
        }
        Ok(update)
    }

    fn install_node_changes(
        &mut self,
        candidate: BTreeMap<TopicPath, Node>,
        changes: Vec<Change>,
        now: Timestamp,
    ) -> Result<(Option<UpdateBatch>, Vec<Diagnostic>), CoreError> {
        let mut candidate_links = self.links.clone();
        let mut warnings = Vec::new();
        let mut changes = reconcile_link_views(
            (&self.nodes, &self.links),
            &candidate,
            &mut candidate_links,
            &self.schemas,
            changes,
            &mut warnings,
            false,
        );
        let candidate_diagnostics = active_diagnostics(
            &candidate,
            &candidate_links,
            &self.schemas,
            &self.diagnostics,
            now,
        );
        changes.extend(diff_visible_nodes(
            &self.diagnostics,
            &candidate_diagnostics,
        ));
        let update = self.commit(changes, CommitPolicy::IfChanged, |core| {
            core.nodes = candidate;
            core.links = candidate_links;
            core.diagnostics = candidate_diagnostics;
        })?;
        Ok((update, warnings))
    }

    /// All fallible work finishes before installation. Publish only after the
    /// complete candidate and sequence have become authoritative.
    fn commit(
        &mut self,
        changes: Vec<Change>,
        policy: CommitPolicy,
        install: impl FnOnce(&mut Self),
    ) -> Result<Option<UpdateBatch>, CoreError> {
        let sequence = match policy {
            CommitPolicy::IfChanged if changes.is_empty() => None,
            CommitPolicy::Always | CommitPolicy::IfChanged => Some(CommitSequence(
                self.sequence
                    .0
                    .checked_add(1)
                    .ok_or(CoreError::SequenceExhausted)?,
            )),
        };
        install(self);
        let update = sequence.map(|sequence| UpdateBatch { sequence, changes });
        if let Some(update) = &update {
            self.sequence = update.sequence;
            self.publish_update(update);
        }
        Ok(update)
    }
}
