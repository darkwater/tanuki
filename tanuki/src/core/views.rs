//! Linked projections, policy checks, and coherent visible diffs.
use super::*;

pub(super) fn visible_nodes(
    canonical: &BTreeMap<TopicPath, Node>,
    links: &LinkRegistry,
) -> BTreeMap<TopicPath, Node> {
    let mut visible = canonical.clone();
    visible.extend(projected_nodes(canonical, links));
    visible
}

pub(super) fn projected_nodes(
    canonical: &BTreeMap<TopicPath, Node>,
    links: &LinkRegistry,
) -> BTreeMap<TopicPath, Node> {
    links
        .iter()
        .filter(|link| link.enabled())
        .flat_map(|link| {
            canonical.iter().filter_map(move |(topic, node)| {
                link.definition()
                    .to_alias(topic)
                    .map(|alias| (alias, node.clone()))
            })
        })
        .collect()
}

pub(super) fn node_value(node: &Node) -> Option<&Value> {
    match node {
        Node::State(state) => Some(state.current().value()),
        Node::Desired(desired) => desired.current().map(RetainedValue::value),
        Node::Event(_) | Node::Command(_) => None,
    }
}

pub(super) fn validate_link_retained(
    definition: &LinkDefinition,
    canonical: &BTreeMap<TopicPath, Node>,
    schemas: &SchemaRegistry,
) -> (bool, Vec<SchemaIssue>, Option<SchemaIssue>) {
    let mut valid = true;
    let mut warnings = Vec::new();
    let mut denial = None;
    for (topic, node) in canonical {
        let Some(alias) = definition.to_alias(topic) else {
            continue;
        };
        for (enforcement, issue) in schemas.inspect_existing(&alias, node.kind(), node_value(node))
        {
            match enforcement {
                Enforcement::Warn => warnings.push(issue),
                Enforcement::Deny => {
                    valid = false;
                    denial.get_or_insert(issue);
                }
            }
        }
    }
    (valid, warnings, denial)
}

pub(super) fn reconcile_link_views(
    previous: (&BTreeMap<TopicPath, Node>, &LinkRegistry),
    candidate: &BTreeMap<TopicPath, Node>,
    candidate_links: &mut LinkRegistry,
    schemas: &SchemaRegistry,
    canonical_changes: Vec<Change>,
    warnings: &mut Vec<Diagnostic>,
    force_recheck: bool,
) -> Vec<Change> {
    let (previous_nodes, old_links) = previous;
    let old_projected = projected_nodes(previous_nodes, old_links);

    for link in candidate_links.iter_mut() {
        let definition = link.definition().clone();
        let relevant = canonical_changes
            .iter()
            .any(|change| definition.to_alias(change.topic()).is_some());
        if !force_recheck && !relevant {
            continue;
        }

        let (mut valid, issues, mut denial) =
            validate_link_retained(&definition, candidate, schemas);
        warnings.extend(issues.into_iter().map(Diagnostic::SchemaWarning));
        for change in &canonical_changes {
            let Some(alias) = definition.to_alias(change.topic()) else {
                continue;
            };
            let (kind, value) = match change {
                Change::Occurrence { event, .. } => (NodeKind::Event, Some(event.value())),
                Change::Command { command, .. } => (NodeKind::Command, Some(command.value())),
                Change::Upsert { .. } | Change::Removed { .. } => continue,
            };
            for (enforcement, issue) in schemas.inspect_existing(&alias, kind, value) {
                match enforcement {
                    Enforcement::Warn => warnings.push(Diagnostic::SchemaWarning(issue)),
                    Enforcement::Deny => {
                        valid = false;
                        denial.get_or_insert(issue);
                    }
                }
            }
        }

        let was_enabled = old_links
            .get(definition.name())
            .is_some_and(|link| link.enabled());
        debug_assert_eq!(valid, denial.is_none());
        link.set_denial(denial.clone());
        match (was_enabled, valid) {
            (true, false) => warnings.push(Diagnostic::LinkDisabled {
                link: definition.name().clone(),
                issue: denial.expect("an invalid link has a denying schema issue"),
            }),
            (false, true) => warnings.push(Diagnostic::LinkEnabled {
                link: definition.name().clone(),
            }),
            _ => {}
        }
    }

    let new_projected = projected_nodes(candidate, candidate_links);
    let mut changes = canonical_changes;
    let instant_sources = changes
        .iter()
        .filter(|change| matches!(change, Change::Occurrence { .. } | Change::Command { .. }))
        .cloned()
        .collect::<Vec<_>>();
    changes.extend(diff_visible_nodes(&old_projected, &new_projected));
    for link in candidate_links.iter().filter(|link| link.enabled()) {
        for change in &instant_sources {
            let Some(topic) = link.definition().to_alias(change.topic()) else {
                continue;
            };
            match change {
                Change::Occurrence { event, .. } => changes.push(Change::Occurrence {
                    topic,
                    event: event.clone(),
                }),
                Change::Command { command, .. } => changes.push(Change::Command {
                    topic,
                    command: command.clone(),
                }),
                Change::Upsert { .. } | Change::Removed { .. } => {}
            }
        }
    }
    changes
}

pub(super) fn diff_visible_nodes(
    previous: &BTreeMap<TopicPath, Node>,
    candidate: &BTreeMap<TopicPath, Node>,
) -> Vec<Change> {
    previous
        .keys()
        .chain(candidate.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|topic| net_node_change(&topic, previous, candidate))
        .collect()
}
