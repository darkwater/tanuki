//! Managed authority and guarded claim release.
use super::*;

impl Core {
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

        let (update, link_warnings) = self.install_node_changes(candidate, changes, now)?;
        warnings.extend(link_warnings);
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
        let (update, link_warnings) = self.install_node_changes(candidate, changes, now)?;
        warnings.extend(link_warnings);
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

    pub(crate) fn open_session_and_subscribe(
        &mut self,
        client: ClientName,
        selection: Selection,
        capacity: SubscriptionCapacity,
        now: Timestamp,
    ) -> Result<(OpenSessionOutcome, Subscription), CoreError> {
        // Reserve capacity before a replacement changes session authority.
        self.next_subscription_id
            .checked_add(1)
            .ok_or(CoreError::SubscriptionIdExhausted)?;
        let opened = self.open_session(client, now)?;
        let subscription = self.subscribe(selection, capacity)?;
        Ok((opened, subscription))
    }

    pub(super) fn validate_actor(&self, actor: &WriteContext) -> Result<(), CoreError> {
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
}

pub(super) fn collect_session_releases(
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

pub(super) fn clear_claim(node: &mut Node) {
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
