use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{
        FromRequestParts, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as RawJson, json};
use tracing::warn;

use crate::{
    core::{CommitOutcome, Core, CoreError, Diagnostic, Snapshot},
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, InputDefinition, InputKind, Node,
        NonNegativeDuration, RetainedValue, Selection, Selector, Timestamp, TopicPath, WriteBatch,
        WriteContext, WriteOperation, WriteProvenance,
    },
    protocol::{JsonValue, encode},
};

pub type SharedCore = Arc<Mutex<Core>>;
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

#[derive(Clone)]
struct HttpState {
    core: SharedCore,
    clock: Clock,
}

pub fn router_with_clock(core: SharedCore, clock: Clock) -> Router {
    Router::new()
        .route("/v1/write", post(batch_write))
        .route("/v1/state/{*topic}", post(single_state_write))
        .route("/v1/snapshot", get(snapshot))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(HttpState { core, clock })
}

async fn not_found() -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        code: "not_found",
        message: "the requested endpoint does not exist".to_owned(),
    }
}

async fn method_not_allowed() -> ApiError {
    ApiError {
        status: StatusCode::METHOD_NOT_ALLOWED,
        code: "method_not_allowed",
        message: "the endpoint does not support this HTTP method".to_owned(),
    }
}

struct StatelessActor(WriteContext);

impl<S: Send + Sync> FromRequestParts<S> for StatelessActor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = parts.headers.get("tanuki-client").ok_or_else(|| {
            ApiError::bad_request("missing_client", "the tanuki-client header is required")
        })?;
        let text = header.to_str().map_err(|_| {
            ApiError::bad_request("invalid_client", "tanuki-client must be valid UTF-8")
        })?;
        let client = ClientName::parse(text)
            .map_err(|error| ApiError::bad_request("invalid_client", error.to_string()))?;
        Ok(Self(WriteContext::stateless(client)))
    }
}

#[derive(Deserialize)]
struct BatchRequest {
    operations: Vec<WireOperation>,
}

#[derive(Deserialize)]
struct StateRequest {
    value: JsonValue,
    expiry: WireExpiry,
}

#[derive(Deserialize)]
struct ReadQuery {
    select: Selector,
}

async fn batch_write(
    State(state): State<HttpState>,
    StatelessActor(actor): StatelessActor,
    payload: Result<Json<BatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<RawJson>>, ApiError> {
    let Json(payload) = payload.map_err(ApiError::invalid_json)?;
    let operations = payload
        .operations
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>, _>>()?;
    apply(&state, actor, operations)
}

async fn single_state_write(
    State(state): State<HttpState>,
    StatelessActor(actor): StatelessActor,
    Path(topic): Path<String>,
    payload: Result<Json<StateRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<RawJson>>, ApiError> {
    let Json(payload) = payload.map_err(ApiError::invalid_json)?;
    let topic = TopicPath::parse(&format!("/{topic}"))
        .map_err(|error| ApiError::bad_request("invalid_topic", error.to_string()))?;
    apply(
        &state,
        actor,
        vec![WriteOperation::PublishState {
            topic,
            value: payload.value.into_inner(),
            expiry: payload.expiry.try_into()?,
        }],
    )
}

fn apply(
    state: &HttpState,
    actor: WriteContext,
    operations: Vec<WriteOperation>,
) -> Result<Json<ApiResponse<RawJson>>, ApiError> {
    let batch = WriteBatch::new(operations)
        .map_err(|error| ApiError::bad_request("empty_batch", error.to_string()))?;
    let outcome = state
        .core
        .lock()
        .map_err(ApiError::poisoned)?
        .apply(&actor, batch, (state.clock)())
        .map_err(ApiError::from_core)?;
    for diagnostic in outcome.warnings() {
        warn!(?diagnostic, client = %actor.client(), "write accepted with diagnostic");
    }
    Ok(Json(ApiResponse::success(commit_json(&outcome))))
}

async fn snapshot(
    State(state): State<HttpState>,
    query: Result<Query<ReadQuery>, QueryRejection>,
) -> Result<Json<ApiResponse<RawJson>>, ApiError> {
    let Query(query) =
        query.map_err(|error| ApiError::bad_request("invalid_query", error.body_text()))?;
    let snapshot = state
        .core
        .lock()
        .map_err(ApiError::poisoned)?
        .read(&Selection::new(vec![query.select]));
    Ok(Json(ApiResponse::success(snapshot_json(&snapshot))))
}

#[derive(Serialize)]
struct ApiResponse<T> {
    ok: bool,
    data: T,
}

impl<T> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self { ok: true, data }
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn invalid_json(error: JsonRejection) -> Self {
        Self::bad_request("invalid_json", error.body_text())
    }

    fn poisoned<T>(_error: std::sync::PoisonError<T>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "core state lock was poisoned".to_owned(),
        }
    }

    fn from_core(error: CoreError) -> Self {
        let (status, code) = match &error {
            CoreError::SystemTopic { .. } => (StatusCode::FORBIDDEN, "system_topic"),
            CoreError::SessionExpired { .. } => (StatusCode::CONFLICT, "session_expired"),
            CoreError::ClaimAuthorityRequired { .. } => {
                (StatusCode::CONFLICT, "claim_authority_required")
            }
            CoreError::SequenceExhausted
            | CoreError::SessionIdExhausted
            | CoreError::ClaimIdExhausted => {
                (StatusCode::INTERNAL_SERVER_ERROR, "capacity_exhausted")
            }
            CoreError::DuplicateTarget { .. } => (StatusCode::BAD_REQUEST, "duplicate_target"),
            CoreError::UnsupportedOperation { .. } => {
                (StatusCode::NOT_IMPLEMENTED, "unsupported_operation")
            }
            CoreError::DeadlineOutOfRange { .. } => {
                (StatusCode::BAD_REQUEST, "deadline_out_of_range")
            }
            CoreError::SessionRequired { .. } => (StatusCode::BAD_REQUEST, "session_required"),
            CoreError::InputNotDefined { .. } => (StatusCode::NOT_FOUND, "input_not_defined"),
            CoreError::NotAnInput { .. } | CoreError::WrongInputKind { .. } => {
                (StatusCode::CONFLICT, "wrong_node_kind")
            }
        };
        Self {
            status,
            code,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            warn!(code = self.code, message = %self.message, "HTTP request failed");
        }
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": {"code": self.code, "message": self.message}
            })),
        )
            .into_response()
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WireOperation {
    PublishState {
        topic: TopicPath,
        value: JsonValue,
        expiry: WireExpiry,
    },
    PublishEvent {
        topic: TopicPath,
        value: JsonValue,
    },
    DefineInput {
        topic: TopicPath,
        kind: WireInputKind,
    },
    ClaimInput {
        topic: TopicPath,
        release: WireRelease,
    },
    SubmitDesired {
        topic: TopicPath,
        value: JsonValue,
        expiry: WireExpiry,
    },
    SubmitCommand {
        topic: TopicPath,
        value: JsonValue,
    },
    ClearDesired {
        topic: TopicPath,
    },
    RemoveNode {
        topic: TopicPath,
    },
}

impl TryFrom<WireOperation> for WriteOperation {
    type Error = ApiError;

    fn try_from(value: WireOperation) -> Result<Self, Self::Error> {
        Ok(match value {
            WireOperation::PublishState {
                topic,
                value,
                expiry,
            } => Self::PublishState {
                topic,
                value: value.into_inner(),
                expiry: expiry.try_into()?,
            },
            WireOperation::PublishEvent { topic, value } => Self::PublishEvent {
                topic,
                value: value.into_inner(),
            },
            WireOperation::DefineInput { topic, kind } => Self::DefineInput {
                topic,
                kind: kind.into(),
                definition: InputDefinition::new(),
            },
            WireOperation::ClaimInput { topic, release } => Self::ClaimInput {
                topic,
                release: release.try_into()?,
            },
            WireOperation::SubmitDesired {
                topic,
                value,
                expiry,
            } => Self::SubmitDesired {
                topic,
                value: value.into_inner(),
                expiry: expiry.try_into()?,
            },
            WireOperation::SubmitCommand { topic, value } => Self::SubmitCommand {
                topic,
                value: value.into_inner(),
            },
            WireOperation::ClearDesired { topic } => Self::ClearDesired { topic },
            WireOperation::RemoveNode { topic } => Self::RemoveNode { topic },
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireInputKind {
    Desired,
    Command,
}

impl From<WireInputKind> for InputKind {
    fn from(value: WireInputKind) -> Self {
        match value {
            WireInputKind::Desired => Self::Desired,
            WireInputKind::Command => Self::Command,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum WireExpiry {
    Preserve,
    Clear,
    Set { duration: String },
}

impl TryFrom<WireExpiry> for ExpiryUpdate {
    type Error = ApiError;

    fn try_from(value: WireExpiry) -> Result<Self, Self::Error> {
        Ok(match value {
            WireExpiry::Preserve => Self::Preserve,
            WireExpiry::Clear => Self::Clear,
            WireExpiry::Set { duration } => Self::Set(nonnegative_duration(&duration)?),
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum WireRelease {
    Immediate,
    After { duration: String },
}

impl TryFrom<WireRelease> for ClaimRelease {
    type Error = ApiError;

    fn try_from(value: WireRelease) -> Result<Self, Self::Error> {
        Ok(match value {
            WireRelease::Immediate => Self::Immediate,
            WireRelease::After { duration } => Self::After(nonnegative_duration(&duration)?),
        })
    }
}

fn nonnegative_duration(input: &str) -> Result<NonNegativeDuration, ApiError> {
    let duration = input.parse().map_err(|error: jiff::Error| {
        ApiError::bad_request("invalid_duration", error.to_string())
    })?;
    NonNegativeDuration::new(duration)
        .map_err(|error| ApiError::bad_request("negative_duration", error.to_string()))
}

fn commit_json(outcome: &CommitOutcome) -> RawJson {
    json!({
        "sequence": outcome.update().sequence().get(),
        "warnings": outcome
            .warnings()
            .iter()
            .map(diagnostic_json)
            .collect::<Vec<_>>()
    })
}

fn diagnostic_json(diagnostic: &Diagnostic) -> RawJson {
    match diagnostic {
        Diagnostic::OutputOwnerChanged {
            topic,
            previous,
            replacement,
        } => {
            json!({"code": "output_owner_changed", "topic": topic, "previous": previous, "replacement": replacement})
        }
        Diagnostic::NodeKindChanged {
            topic,
            previous,
            replacement,
        } => {
            json!({"code": "node_kind_changed", "topic": topic, "previous": format!("{previous:?}").to_lowercase(), "replacement": format!("{replacement:?}").to_lowercase()})
        }
        Diagnostic::NodeAlreadyAbsent { topic } => {
            json!({"code": "node_already_absent", "topic": topic})
        }
        Diagnostic::SessionReplaced {
            client,
            previous,
            replacement,
        } => {
            json!({"code": "session_replaced", "client": client, "previous": previous.get(), "replacement": replacement.get()})
        }
        Diagnostic::InputClaimReplaced {
            topic,
            previous,
            replacement,
        } => {
            json!({"code": "input_claim_replaced", "topic": topic, "previous": previous, "replacement": replacement})
        }
        Diagnostic::ClaimGraceOutOfRange { topic } => {
            json!({"code": "claim_grace_out_of_range", "topic": topic})
        }
    }
}

fn snapshot_json(snapshot: &Snapshot) -> RawJson {
    let nodes: JsonMap<_, _> = snapshot
        .nodes()
        .iter()
        .map(|(topic, node)| (topic.to_string(), node_json(node)))
        .collect();
    json!({"sequence": snapshot.sequence().get(), "nodes": nodes})
}

fn node_json(node: &Node) -> RawJson {
    match node {
        Node::State(node) => retained_node_json("state", node.current()),
        Node::Event(node) => {
            json!({"kind": "event", "last_write": provenance_json(node.last_publisher())})
        }
        Node::Desired(node) => json!({
            "kind": "desired",
            "value": node.current().map(|current| encode(current.value())),
            "last_write": node.current().map(|current| provenance_json(current.last_write())),
            "expires_at": node.current().and_then(RetainedValue::expires_at).map(|deadline| deadline.get().get().to_string()),
            "claim": node.claim().map(|claim| json!({"owner": claim.owner(), "session": claim.session().get(), "id": claim.id().get()}))
        }),
        Node::Command(node) => json!({
            "kind": "command",
            "claim": node.claim().map(|claim| json!({"owner": claim.owner(), "session": claim.session().get(), "id": claim.id().get()}))
        }),
    }
}

fn retained_node_json(kind: &str, retained: &RetainedValue) -> RawJson {
    json!({
        "kind": kind,
        "value": encode(retained.value()),
        "last_write": provenance_json(retained.last_write()),
        "expires_at": retained.expires_at().map(|deadline| deadline.get().get().to_string())
    })
}

fn provenance_json(provenance: &WriteProvenance) -> RawJson {
    json!({
        "client": provenance.client(),
        "session": provenance.session().map(|session| session.get()),
        "at": provenance.at().get().to_string()
    })
}
