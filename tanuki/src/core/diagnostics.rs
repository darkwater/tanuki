//! Derived active freshness and disabled-link conditions.
use super::*;

pub(super) fn freshness_deadline(
    topic: &TopicPath,
    node: &Node,
    schemas: &SchemaRegistry,
) -> Option<Deadline> {
    let current = node.retained_value()?;
    let interval = schemas.expected_update_interval(topic)?;
    current
        .last_write()
        .at()
        .get()
        .checked_add(interval.get())
        .ok()
        .map(|timestamp| Deadline::new(Timestamp::new(timestamp)))
}

pub(super) fn freshness_diagnostic_topic(topic: &TopicPath) -> TopicPath {
    TopicPath::parse(&format!("/$diagnostics/freshness{topic}"))
        .expect("a validated topic remains valid under the diagnostics prefix")
}

pub(super) fn active_diagnostics(
    canonical: &BTreeMap<TopicPath, Node>,
    links: &LinkRegistry,
    schemas: &SchemaRegistry,
    previous: &BTreeMap<TopicPath, Node>,
    now: Timestamp,
) -> BTreeMap<TopicPath, Node> {
    let mut diagnostics = visible_nodes(canonical, links)
        .into_iter()
        .filter_map(|(topic, node)| {
            let current = node.retained_value()?;
            let interval = schemas.expected_update_interval(&topic)?;
            let deadline = freshness_deadline(&topic, &node, schemas)?;
            if deadline.get() > now {
                return None;
            }
            let mut value = BTreeMap::new();
            value.insert(
                "code".to_owned(),
                Value::String("overdue_update".to_owned()),
            );
            value.insert("topic".to_owned(), Value::String(topic.to_string()));
            value.insert(
                "expected_update_interval".to_owned(),
                Value::Duration(interval.get()),
            );
            value.insert(
                "last_updated".to_owned(),
                Value::Timestamp(current.last_write().at().get()),
            );
            let path = freshness_diagnostic_topic(&topic);
            Some((
                path.clone(),
                active_condition_node(&path, Value::Map(value), previous, deadline.get()),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    for link in links.iter().filter(|link| !link.enabled()) {
        let Some(issue) = link.denial() else {
            continue;
        };
        let path = TopicPath::parse(&format!(
            "/$diagnostics/links/{}",
            link.definition().name().as_str()
        ))
        .expect("a validated link name is a valid diagnostic path segment");
        let mut value = BTreeMap::new();
        value.insert("code".to_owned(), Value::String("link_disabled".to_owned()));
        value.insert(
            "link".to_owned(),
            Value::String(link.definition().name().as_str().to_owned()),
        );
        value.insert("topic".to_owned(), Value::String(issue.topic().to_string()));
        value.insert(
            "message".to_owned(),
            Value::String(issue.kind().to_string()),
        );
        diagnostics.insert(
            path.clone(),
            active_condition_node(&path, Value::Map(value), previous, now),
        );
    }
    diagnostics
}

pub(super) fn active_condition_node(
    path: &TopicPath,
    value: Value,
    previous: &BTreeMap<TopicPath, Node>,
    at: Timestamp,
) -> Node {
    if let Some(Node::State(existing)) = previous.get(path)
        && existing.current().value() == &value
    {
        return Node::State(existing.clone());
    }
    let context = WriteContext::stateless(
        ClientName::parse("tanuki").expect("the built-in client name is valid"),
    );
    Node::State(StateNode::new(RetainedValue::new(
        value,
        WriteProvenance::from_context(&context, at),
        None,
    )))
}
