//! Operation validation and candidate mutation.
use super::*;

pub(super) fn validate_operation(
    schemas: &SchemaRegistry,
    operation: &WriteOperation,
    warnings: &mut Vec<Diagnostic>,
) -> Result<WriteOperation, CoreError> {
    let validate = |topic: &TopicPath, value: &Value| {
        schemas
            .validate_value(topic, value.clone())
            .map(|validated| validated.into_parts())
            .map_err(CoreError::from)
    };
    let validate_kind = |topic: &TopicPath, node_kind: NodeKind| {
        schemas
            .validate_node_kind(topic, node_kind)
            .map_err(CoreError::from)
    };
    let operation = match operation {
        WriteOperation::PublishState {
            topic,
            value,
            expiry,
        } => {
            warnings.extend(
                validate_kind(topic, NodeKind::State)?
                    .into_iter()
                    .map(Diagnostic::SchemaWarning),
            );
            let (value, issues) = validate(topic, value)?;
            warnings.extend(issues.into_iter().map(Diagnostic::SchemaWarning));
            WriteOperation::PublishState {
                topic: topic.clone(),
                value,
                expiry: *expiry,
            }
        }
        WriteOperation::PublishEvent { topic, value } => {
            warnings.extend(
                validate_kind(topic, NodeKind::Event)?
                    .into_iter()
                    .map(Diagnostic::SchemaWarning),
            );
            let (value, issues) = validate(topic, value)?;
            warnings.extend(issues.into_iter().map(Diagnostic::SchemaWarning));
            WriteOperation::PublishEvent {
                topic: topic.clone(),
                value,
            }
        }
        WriteOperation::SubmitDesired {
            topic,
            value,
            expiry,
        } => {
            warnings.extend(
                validate_kind(topic, NodeKind::Desired)?
                    .into_iter()
                    .map(Diagnostic::SchemaWarning),
            );
            let (value, issues) = validate(topic, value)?;
            warnings.extend(issues.into_iter().map(Diagnostic::SchemaWarning));
            WriteOperation::SubmitDesired {
                topic: topic.clone(),
                value,
                expiry: *expiry,
            }
        }
        WriteOperation::SubmitCommand { topic, value } => {
            warnings.extend(
                validate_kind(topic, NodeKind::Command)?
                    .into_iter()
                    .map(Diagnostic::SchemaWarning),
            );
            let (value, issues) = validate(topic, value)?;
            warnings.extend(issues.into_iter().map(Diagnostic::SchemaWarning));
            WriteOperation::SubmitCommand {
                topic: topic.clone(),
                value,
            }
        }
        WriteOperation::DefineInput {
            topic,
            kind,
            definition,
        } => {
            warnings.extend(
                validate_kind(topic, node_kind(*kind))?
                    .into_iter()
                    .map(Diagnostic::SchemaWarning),
            );
            WriteOperation::DefineInput {
                topic: topic.clone(),
                kind: *kind,
                definition: definition.clone(),
            }
        }
        operation => operation.clone(),
    };
    Ok(operation)
}

pub(super) fn prepare_alias_operation(
    links: &LinkRegistry,
    schemas: &SchemaRegistry,
    operation: &WriteOperation,
    warnings: &mut Vec<Diagnostic>,
) -> Result<WriteOperation, CoreError> {
    let Some((_, canonical)) = links.resolve_alias(operation.topic()) else {
        return Ok(operation.clone());
    };
    let operation = validate_operation(schemas, operation, warnings)?;
    Ok(with_operation_topic(operation, canonical))
}

pub(super) fn with_operation_topic(operation: WriteOperation, topic: TopicPath) -> WriteOperation {
    match operation {
        WriteOperation::PublishState { value, expiry, .. } => WriteOperation::PublishState {
            topic,
            value,
            expiry,
        },
        WriteOperation::PublishEvent { value, .. } => WriteOperation::PublishEvent { topic, value },
        WriteOperation::DefineInput {
            kind, definition, ..
        } => WriteOperation::DefineInput {
            topic,
            kind,
            definition,
        },
        WriteOperation::ClaimInput { release, .. } => WriteOperation::ClaimInput { topic, release },
        WriteOperation::SubmitDesired { value, expiry, .. } => WriteOperation::SubmitDesired {
            topic,
            value,
            expiry,
        },
        WriteOperation::SubmitCommand { value, .. } => {
            WriteOperation::SubmitCommand { topic, value }
        }
        WriteOperation::ClearDesired { .. } => WriteOperation::ClearDesired { topic },
        WriteOperation::RemoveNode { .. } => WriteOperation::RemoveNode { topic },
    }
}

pub(super) fn operation_value_kind(operation: &WriteOperation) -> Option<crate::domain::ValueKind> {
    match operation {
        WriteOperation::PublishState { value, .. }
        | WriteOperation::PublishEvent { value, .. }
        | WriteOperation::SubmitDesired { value, .. }
        | WriteOperation::SubmitCommand { value, .. } => Some(value.kind()),
        WriteOperation::DefineInput { .. }
        | WriteOperation::ClaimInput { .. }
        | WriteOperation::ClearDesired { .. }
        | WriteOperation::RemoveNode { .. } => None,
    }
}

pub(super) fn validate_topics(batch: &WriteBatch) -> Result<(), CoreError> {
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

pub(super) fn nodes_have_no_denying_schema_violations(
    schemas: &SchemaRegistry,
    nodes: &BTreeMap<TopicPath, Node>,
) -> bool {
    nodes.iter().all(|(topic, node)| {
        let value = match node {
            Node::State(state) => Some(state.current().value()),
            Node::Desired(desired) => desired.current().map(RetainedValue::value),
            Node::Event(_) | Node::Command(_) => None,
        };
        schemas
            .inspect_existing(topic, node.kind(), value)
            .into_iter()
            .all(|(enforcement, _)| enforcement != Enforcement::Deny)
    })
}

pub(super) fn coalesce_batch_changes(
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

pub(super) fn net_node_change(
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

pub(super) fn apply_operation(
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

pub(super) fn warn_on_output_replacement(
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

pub(super) fn warn_on_kind_change(
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

pub(super) fn input_claim(node: Option<&Node>) -> Option<&InputClaim> {
    match node {
        Some(Node::Desired(node)) => node.claim(),
        Some(Node::Command(node)) => node.claim(),
        Some(Node::State(_) | Node::Event(_)) | None => None,
    }
}

pub(super) fn resolve_expiry(
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
