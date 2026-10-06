//! Decode payload-bearing tagged enums through streaming struct fields.
//!
//! Serde's internally tagged enum derive buffers into ContentDeserializer, which
//! defaults to human-readable and loses MessagePack bytes/extensions and literal
//! tag-looking maps. Struct derives keep the original decoder for each value and
//! allow the discriminator anywhere in a map. Public enums remain the validated
//! shape; these permissive, private field bags exist only during decoding.
use crate::*;
use serde::{Deserialize, Deserializer, de};
use std::collections::BTreeMap;

fn required<T, E: de::Error>(value: Option<T>, field: &'static str) -> Result<T, E> {
    value.ok_or_else(|| E::missing_field(field))
}
// An optional field's presence differs from a payload which is explicitly null.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(decoder).map(Some)
}

#[derive(Deserialize)]
struct ClientFields {
    #[serde(rename = "type")]
    kind: String,
    request_id: RequestId,
    client: Option<ClientName>,
    selectors: Option<Vec<Selector>>,
    operations: Option<Vec<WireOperation>>,
}
impl<'de> Deserialize<'de> for ClientMessage {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let f = ClientFields::deserialize(decoder)?;
        match f.kind.as_str() {
            "hello" => Ok(Self::Hello {
                request_id: f.request_id,
                client: required(f.client, "client")?,
                selectors: required(f.selectors, "selectors")?,
            }),
            "write" => Ok(Self::Write {
                request_id: f.request_id,
                operations: required(f.operations, "operations")?,
            }),
            other => Err(de::Error::unknown_variant(other, &["hello", "write"])),
        }
    }
}
#[derive(Deserialize)]
struct OperationFields {
    op: String,
    topic: TopicPath,
    #[serde(default, deserialize_with = "present")]
    value: Option<JsonValue>,
    expiry: Option<WireExpiry>,
    kind: Option<WireInputKind>,
    release: Option<WireRelease>,
}
impl<'de> Deserialize<'de> for WireOperation {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let f = OperationFields::deserialize(decoder)?;
        match f.op.as_str() {
            "publish_state" => Ok(Self::PublishState {
                topic: f.topic,
                value: required(f.value, "value")?,
                expiry: f.expiry,
            }),
            "publish_event" => Ok(Self::PublishEvent {
                topic: f.topic,
                value: required(f.value, "value")?,
            }),
            "define_input" => Ok(Self::DefineInput {
                topic: f.topic,
                kind: required(f.kind, "kind")?,
            }),
            "claim_input" => Ok(Self::ClaimInput {
                topic: f.topic,
                release: required(f.release, "release")?,
            }),
            "submit_desired" => Ok(Self::SubmitDesired {
                topic: f.topic,
                value: required(f.value, "value")?,
                expiry: f.expiry,
            }),
            "submit_command" => Ok(Self::SubmitCommand {
                topic: f.topic,
                value: required(f.value, "value")?,
            }),
            "clear_desired" => Ok(Self::ClearDesired { topic: f.topic }),
            "remove_node" => Ok(Self::RemoveNode { topic: f.topic }),
            other => Err(de::Error::unknown_variant(
                other,
                &[
                    "publish_state",
                    "publish_event",
                    "define_input",
                    "claim_input",
                    "submit_desired",
                    "submit_command",
                    "clear_desired",
                    "remove_node",
                ],
            )),
        }
    }
}
#[derive(Deserialize)]
struct NodeFields {
    kind: String,
    current: Option<CurrentValueView>,
    last_publisher: Option<ProvenanceView>,
    definition: Option<InputDefinitionView>,
    claim: Option<ClaimView>,
}
impl<'de> Deserialize<'de> for NodeView {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let f = NodeFields::deserialize(decoder)?;
        match f.kind.as_str() {
            "state" => Ok(Self::State {
                current: required(f.current, "current")?,
            }),
            "event" => Ok(Self::Event {
                last_publisher: required(f.last_publisher, "last_publisher")?,
            }),
            "desired" => Ok(Self::Desired {
                definition: required(f.definition, "definition")?,
                claim: f.claim,
                current: f.current,
            }),
            "command" => Ok(Self::Command {
                definition: required(f.definition, "definition")?,
                claim: f.claim,
            }),
            other => Err(de::Error::unknown_variant(
                other,
                &["state", "event", "desired", "command"],
            )),
        }
    }
}
#[derive(Deserialize)]
struct ChangeFields {
    #[serde(rename = "type")]
    kind: String,
    topic: String,
    node: Option<NodeView>,
    previous: Option<NodeView>,
    event: Option<OccurrenceView>,
    command: Option<OccurrenceView>,
}
impl<'de> Deserialize<'de> for ChangeView {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let f = ChangeFields::deserialize(decoder)?;
        match f.kind.as_str() {
            "upsert" => Ok(Self::Upsert {
                topic: f.topic,
                node: required(f.node, "node")?,
            }),
            "removed" => Ok(Self::Removed {
                topic: f.topic,
                previous: required(f.previous, "previous")?,
            }),
            "event" => Ok(Self::Event {
                topic: f.topic,
                event: required(f.event, "event")?,
            }),
            "command" => Ok(Self::Command {
                topic: f.topic,
                command: required(f.command, "command")?,
            }),
            other => Err(de::Error::unknown_variant(
                other,
                &["upsert", "removed", "event", "command"],
            )),
        }
    }
}
#[derive(Deserialize)]
struct ServerFields {
    #[serde(rename = "type")]
    kind: String,
    request_id: Option<RequestId>,
    sequence: Option<u64>,
    nodes: Option<BTreeMap<String, NodeView>>,
    warnings: Option<Vec<DiagnosticView>>,
    changes: Option<Vec<ChangeView>>,
    #[serde(default, deserialize_with = "present")]
    result: Option<serde_json::Value>,
    error: Option<ErrorView>,
}
impl<'de> Deserialize<'de> for ServerMessage {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let f = ServerFields::deserialize(decoder)?;
        match f.kind.as_str() {
            "snapshot" => Ok(Self::Snapshot {
                request_id: required(f.request_id, "request_id")?,
                sequence: required(f.sequence, "sequence")?,
                nodes: required(f.nodes, "nodes")?,
                warnings: required(f.warnings, "warnings")?,
            }),
            "update" => Ok(Self::Update {
                sequence: required(f.sequence, "sequence")?,
                changes: required(f.changes, "changes")?,
            }),
            "reply" => Ok(Self::Reply {
                request_id: required(f.request_id, "request_id")?,
                result: required(f.result, "result")?,
            }),
            "error" => Ok(Self::Error {
                request_id: f.request_id,
                error: required(f.error, "error")?,
            }),
            other => Err(de::Error::unknown_variant(
                other,
                &["snapshot", "update", "reply", "error"],
            )),
        }
    }
}
