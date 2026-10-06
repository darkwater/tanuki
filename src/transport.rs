use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{
        FromRequestParts, Path, Query, State, WebSocketUpgrade,
        rejection::{JsonRejection, QueryRejection},
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as RawJson, json};
use tokio::sync::watch;
use tracing::warn;

use crate::{
    core::{
        CommitOutcome, Core, CoreError, Diagnostic, SchemaInstallMode, Subscription,
        SubscriptionCapacity, SubscriptionEnd,
    },
    domain::{
        ClaimRelease, ClientName, ExpiryUpdate, FiniteF64, InputDefinition, InputKind, NodeKind,
        NonNegativeDuration, Selection, Selector, SessionHandle, SessionId, Timestamp, TopicPath,
        ValueKind, WriteBatch, WriteContext, WriteOperation,
    },
    protocol::{ErrorView, JsonValue, RequestId, ServerMessage, SnapshotView},
    scheduler::DeadlineScheduler,
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueCast, ValueValidator},
};

pub type SharedCore = Arc<Mutex<Core>>;
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

#[derive(Clone)]
struct HttpState {
    core: SharedCore,
    clock: Clock,
    scheduler: DeadlineScheduler,
    connections: Arc<Mutex<BTreeMap<ClientName, ActiveConnection>>>,
}

#[derive(Debug)]
struct ActiveConnection {
    session: SessionId,
    kick: watch::Sender<bool>,
}

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const SUBSCRIPTION_BATCH_CAPACITY: usize = 64;

pub fn router_with_clock(core: SharedCore, clock: Clock) -> Router {
    let scheduler = DeadlineScheduler::start(core.clone(), clock.clone());
    Router::new()
        .route("/v1/write", post(batch_write))
        .route("/v1/state/{*topic}", post(single_state_write))
        .route("/v1/schemas/{name}", put(install_schema))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/ws", get(websocket_upgrade))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(HttpState {
            core,
            clock,
            scheduler,
            connections: Arc::new(Mutex::new(BTreeMap::new())),
        })
}

#[derive(Deserialize)]
struct SchemaRequest {
    #[serde(default)]
    force: bool,
    rules: Vec<SchemaRuleRequest>,
}

#[derive(Deserialize)]
struct SchemaRuleRequest {
    selector: Selector,
    enforcement: EnforcementRequest,
    #[serde(default)]
    nullable: bool,
    validator: ValidatorRequest,
    cast: Option<CastRequest>,
    node_kind: Option<NodeKindRequest>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EnforcementRequest {
    Warn,
    Deny,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ValidatorRequest {
    Any,
    Kind { kind: ValueKindRequest },
    IntegerRange { minimum: i64, maximum: i64 },
    FloatRange { minimum: f64, maximum: f64 },
    StringEnum { values: Vec<String> },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValueKindRequest {
    Bool,
    Integer,
    Float,
    String,
    Bytes,
    List,
    Map,
    Timestamp,
    Duration,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CastRequest {
    #[serde(rename = "string_to_integer")]
    Integer,
    #[serde(rename = "string_to_float")]
    Float,
    #[serde(rename = "string_to_bool")]
    Bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NodeKindRequest {
    State,
    Event,
    Desired,
    Command,
}

async fn install_schema(
    State(state): State<HttpState>,
    Path(name): Path<String>,
    Json(request): Json<SchemaRequest>,
) -> Result<Json<RawJson>, ApiError> {
    let schema = build_schema(&name, request.rules)?;
    let mode = if request.force {
        SchemaInstallMode::RemoveInvalid
    } else {
        SchemaInstallMode::RejectInvalid
    };
    let outcome = state
        .core
        .lock()
        .map_err(ApiError::poisoned)?
        .install_schema(schema, mode)
        .map_err(ApiError::from_core)?;
    if outcome.update().is_some() {
        state.scheduler.rescan();
    }
    for warning in outcome.warnings() {
        warn!(?warning, schema = %name, "schema installed with diagnostic");
    }
    Ok(Json(json!({
        "schema": name,
        "sequence": outcome.update().map(|update| update.sequence().get()),
        "warnings": outcome.warnings().iter().map(diagnostic_json).collect::<Vec<_>>(),
    })))
}

fn build_schema(name: &str, rules: Vec<SchemaRuleRequest>) -> Result<Schema, ApiError> {
    let name = SchemaName::parse(name)
        .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))?;
    let rules = rules
        .into_iter()
        .map(build_schema_rule)
        .collect::<Result<Vec<_>, _>>()?;
    Schema::new(name, rules)
        .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))
}

fn build_schema_rule(request: SchemaRuleRequest) -> Result<SchemaRule, ApiError> {
    let null_policy = if request.nullable {
        NullPolicy::Allow
    } else {
        NullPolicy::Deny
    };
    let validator = match request.validator {
        ValidatorRequest::Any => Ok(ValueValidator::any(null_policy)),
        ValidatorRequest::Kind { kind } => ValueValidator::kind(kind.into(), null_policy),
        ValidatorRequest::IntegerRange { minimum, maximum } => {
            ValueValidator::integer_range(minimum, maximum, null_policy)
        }
        ValidatorRequest::FloatRange { minimum, maximum } => {
            let minimum = FiniteF64::new(minimum)
                .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))?;
            let maximum = FiniteF64::new(maximum)
                .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))?;
            ValueValidator::float_range(minimum, maximum, null_policy)
        }
        ValidatorRequest::StringEnum { values } => ValueValidator::string_enum(values, null_policy),
    }
    .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))?;
    let rule = SchemaRule::new(request.selector, request.enforcement.into(), validator)
        .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))?;
    let rule = match request.cast {
        Some(cast) => rule
            .with_cast(cast.into())
            .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string())),
        None => Ok(rule),
    }?;
    Ok(match request.node_kind {
        Some(node_kind) => rule.with_node_kind(node_kind.into()),
        None => rule,
    })
}

impl From<EnforcementRequest> for Enforcement {
    fn from(value: EnforcementRequest) -> Self {
        match value {
            EnforcementRequest::Warn => Self::Warn,
            EnforcementRequest::Deny => Self::Deny,
        }
    }
}

impl From<ValueKindRequest> for ValueKind {
    fn from(value: ValueKindRequest) -> Self {
        match value {
            ValueKindRequest::Bool => Self::Bool,
            ValueKindRequest::Integer => Self::Integer,
            ValueKindRequest::Float => Self::Float,
            ValueKindRequest::String => Self::String,
            ValueKindRequest::Bytes => Self::Bytes,
            ValueKindRequest::List => Self::List,
            ValueKindRequest::Map => Self::Map,
            ValueKindRequest::Timestamp => Self::Timestamp,
            ValueKindRequest::Duration => Self::Duration,
        }
    }
}

impl From<CastRequest> for ValueCast {
    fn from(value: CastRequest) -> Self {
        match value {
            CastRequest::Integer => Self::StringToInteger,
            CastRequest::Float => Self::StringToFloat,
            CastRequest::Bool => Self::StringToBool,
        }
    }
}

impl From<NodeKindRequest> for NodeKind {
    fn from(value: NodeKindRequest) -> Self {
        match value {
            NodeKindRequest::State => Self::State,
            NodeKindRequest::Event => Self::Event,
            NodeKindRequest::Desired => Self::Desired,
            NodeKindRequest::Command => Self::Command,
        }
    }
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

async fn websocket_upgrade(State(state): State<HttpState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| websocket_session(socket, state))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Hello {
        request_id: RequestId,
        client: ClientName,
        selectors: Vec<Selector>,
    },
    Write {
        request_id: RequestId,
        operations: Vec<WireOperation>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WireCodec {
    Json,
    MessagePack,
}

async fn websocket_session(mut socket: WebSocket, state: HttpState) {
    let Some(first) = socket.recv().await else {
        return;
    };
    let Ok(first) = first else {
        return;
    };
    let (codec, first) = match decode_client_message(first) {
        Ok(decoded) => decoded,
        Err((codec, error)) => {
            let _ = send_server_message(
                &mut socket,
                codec,
                &error_message(None, "invalid_message", error),
            )
            .await;
            return;
        }
    };
    let ClientMessage::Hello {
        request_id,
        client,
        selectors,
    } = first
    else {
        let _ = send_server_message(
            &mut socket,
            codec,
            &error_message(
                None,
                "hello_required",
                "the first WebSocket message must be hello",
            ),
        )
        .await;
        return;
    };

    let opened = match state
        .core
        .lock()
        .map_err(ApiError::poisoned)
        .and_then(|mut core| {
            let opened = core
                .open_session(client.clone(), (state.clock)())
                .map_err(ApiError::from_core)?;
            let subscription = core
                .subscribe(
                    Selection::new(selectors),
                    SubscriptionCapacity::new(SUBSCRIPTION_BATCH_CAPACITY)
                        .expect("configured subscription capacity is nonzero"),
                )
                .map_err(ApiError::from_core)?;
            Ok((opened, subscription))
        }) {
        Ok(value) => value,
        Err(api) => {
            let _ =
                send_server_message(&mut socket, codec, &api_server_error(Some(request_id), api))
                    .await;
            return;
        }
    };
    let (opened, mut subscription) = opened;
    let handle = opened.handle().clone();
    for warning in opened.warnings() {
        warn!(?warning, client = %handle.client(), "WebSocket session opened with diagnostic");
    }
    for release in opened.pending_releases() {
        warn!(topic = %release.topic(), claim = release.claim().get(), "replacement claim release scheduled");
    }
    state.scheduler.schedule_claims(opened.pending_releases());
    let (kick, mut kicked) = watch::channel(false);
    let previous = state
        .connections
        .lock()
        .expect("connection registry lock is not poisoned")
        .insert(
            client.clone(),
            ActiveConnection {
                session: handle.id(),
                kick,
            },
        );
    if let Some(previous) = previous {
        previous.kick.send_replace(true);
    }

    let snapshot = ServerMessage::snapshot(request_id, subscription.snapshot(), opened.warnings());
    if send_server_message(&mut socket, codec, &snapshot)
        .await
        .is_ok()
    {
        websocket_loop(
            &mut socket,
            &state,
            codec,
            &handle,
            &mut subscription,
            &mut kicked,
        )
        .await;
    }
    finish_websocket_session(&state, &handle);
}

async fn websocket_loop(
    socket: &mut WebSocket,
    state: &HttpState,
    subscription_codec: WireCodec,
    handle: &SessionHandle,
    subscription: &mut Subscription,
    kicked: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            result = kicked.changed() => {
                if result.is_ok() && *kicked.borrow() {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: 4001,
                        reason: "session_replaced".into(),
                    }))).await;
                }
                return;
            }
            update = subscription.update() => {
                let Some(update) = update else {
                    if subscription.end_reason() == Some(SubscriptionEnd::SlowConsumer) {
                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: 1013,
                            reason: "slow_consumer".into(),
                        }))).await;
                    }
                    return;
                };
                if send_server_message(socket, subscription_codec, &ServerMessage::update(&update)).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(incoming)) = incoming else { return };
                match incoming {
                    Message::Ping(payload) => {
                        if socket.send(Message::Pong(payload)).await.is_err() { return; }
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => return,
                    Message::Text(_) | Message::Binary(_) => {
                        let decoded = decode_client_message(incoming);
                        let (incoming_codec, message) = match decoded {
                            Ok(decoded) => decoded,
                            Err((incoming_codec, error)) => {
                                if send_server_message(socket, incoming_codec, &error_message(None, "invalid_message", error)).await.is_err() { return; }
                                continue;
                            }
                        };
                        match message {
                            ClientMessage::Hello { request_id, .. } => {
                                if send_server_message(socket, incoming_codec, &error_message(Some(request_id), "already_initialized", "hello is only valid as the first message")).await.is_err() { return; }
                            }
                            ClientMessage::Write { request_id, operations } => {
                                let response = apply_websocket_write(state, handle, request_id, operations);
                                let expired = matches!(&response, ServerMessage::Error { error, .. } if error.code == "session_expired");
                                if send_server_message(socket, incoming_codec, &response).await.is_err() { return; }
                                if expired { return; }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn apply_websocket_write(
    state: &HttpState,
    handle: &SessionHandle,
    request_id: RequestId,
    operations: Vec<WireOperation>,
) -> ServerMessage {
    let operations = match operations
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>, ApiError>>()
    {
        Ok(operations) => operations,
        Err(error) => return api_server_error(Some(request_id), error),
    };
    let batch = match WriteBatch::new(operations) {
        Ok(batch) => batch,
        Err(error) => {
            return error_message(Some(request_id), "empty_batch", error.to_string());
        }
    };
    match state
        .core
        .lock()
        .map_err(ApiError::poisoned)
        .and_then(|mut core| {
            core.apply(
                &WriteContext::managed(handle.clone()),
                batch,
                (state.clock)(),
            )
            .map_err(ApiError::from_core)
        }) {
        Ok(outcome) => {
            state.scheduler.rescan();
            for diagnostic in outcome.warnings() {
                warn!(?diagnostic, client = %handle.client(), "WebSocket write accepted with diagnostic");
            }
            ServerMessage::Reply {
                request_id,
                result: commit_json(&outcome),
            }
        }
        Err(error) => api_server_error(Some(request_id), error),
    }
}

fn finish_websocket_session(state: &HttpState, handle: &SessionHandle) {
    if let Ok(mut core) = state.core.lock()
        && let Ok(outcome) = core.disconnect(handle, (state.clock)())
    {
        for warning in outcome.warnings() {
            warn!(?warning, client = %handle.client(), "disconnect completed with diagnostic");
        }
        for release in outcome.pending_releases() {
            warn!(topic = %release.topic(), claim = release.claim().get(), "claim release scheduled after disconnect");
        }
        state.scheduler.schedule_claims(outcome.pending_releases());
    }
    let mut connections = state
        .connections
        .lock()
        .expect("connection registry lock is not poisoned");
    if connections
        .get(handle.client())
        .is_some_and(|connection| connection.session == handle.id())
    {
        connections.remove(handle.client());
    }
}

fn decode_client_message(
    message: Message,
) -> Result<(WireCodec, ClientMessage), (WireCodec, String)> {
    match message {
        Message::Text(text) => serde_json::from_str(&text)
            .map(|message| (WireCodec::Json, message))
            .map_err(|error| (WireCodec::Json, error.to_string())),
        Message::Binary(bytes) => rmp_serde::from_slice(&bytes)
            .map(|message| (WireCodec::MessagePack, message))
            .map_err(|error| (WireCodec::MessagePack, error.to_string())),
        _ => Err((
            WireCodec::Json,
            "expected a text or binary data message".to_owned(),
        )),
    }
}

async fn send_server_message(
    socket: &mut WebSocket,
    codec: WireCodec,
    message: &ServerMessage,
) -> Result<(), ()> {
    let message = match codec {
        WireCodec::Json => {
            let encoded = serde_json::to_string(message).map_err(|_| ())?;
            if encoded.len() > MAX_MESSAGE_BYTES {
                return Err(());
            }
            Message::Text(encoded.into())
        }
        WireCodec::MessagePack => {
            let encoded = rmp_serde::to_vec_named(message).map_err(|_| ())?;
            if encoded.len() > MAX_MESSAGE_BYTES {
                return Err(());
            }
            Message::Binary(encoded.into())
        }
    };
    socket.send(message).await.map_err(|_| ())
}

fn error_message(
    request_id: Option<RequestId>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ServerMessage {
    ServerMessage::Error {
        request_id,
        error: ErrorView {
            code: code.into(),
            message: message.into(),
        },
    }
}

fn api_server_error(request_id: Option<RequestId>, error: ApiError) -> ServerMessage {
    error_message(request_id, error.code, error.message)
}

struct StatelessActor(WriteContext);

#[derive(Deserialize)]
struct ClientQuery {
    client: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for StatelessActor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("tanuki-client")
            .map(|header| {
                header.to_str().map_err(|_| {
                    ApiError::bad_request("invalid_client", "tanuki-client must be valid UTF-8")
                })
            })
            .transpose()?
            .map(parse_client)
            .transpose()?;
        let Query(query) = Query::<ClientQuery>::try_from_uri(&parts.uri)
            .map_err(|error| ApiError::bad_request("invalid_query", error.body_text()))?;
        let query = query.client.as_deref().map(parse_client).transpose()?;
        if let (Some(header), Some(query)) = (&header, &query)
            && header != query
        {
            return Err(ApiError::bad_request(
                "conflicting_client",
                "tanuki-client and the client query parameter disagree",
            ));
        }
        let client = header.or(query).ok_or_else(|| {
            ApiError::bad_request(
                "missing_client",
                "the tanuki-client header or client query parameter is required",
            )
        })?;
        Ok(Self(WriteContext::stateless(client)))
    }
}

fn parse_client(value: &str) -> Result<ClientName, ApiError> {
    ClientName::parse(value)
        .map_err(|error| ApiError::bad_request("invalid_client", error.to_string()))
}

#[derive(Deserialize)]
struct BatchRequest {
    operations: Vec<WireOperation>,
}

#[derive(Deserialize)]
struct StateRequest {
    value: JsonValue,
    #[serde(default)]
    expiry: Option<WireExpiry>,
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
            expiry: expiry_update(payload.expiry)?,
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
    state.scheduler.rescan();
    Ok(Json(ApiResponse::success(commit_json(&outcome))))
}

async fn snapshot(
    State(state): State<HttpState>,
    query: Result<Query<ReadQuery>, QueryRejection>,
) -> Result<Json<ApiResponse<SnapshotView>>, ApiError> {
    let Query(query) =
        query.map_err(|error| ApiError::bad_request("invalid_query", error.body_text()))?;
    let snapshot = state
        .core
        .lock()
        .map_err(ApiError::poisoned)?
        .read(&Selection::new(vec![query.select]));
    Ok(Json(ApiResponse::success(SnapshotView::from(&snapshot))))
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
            | CoreError::ClaimIdExhausted
            | CoreError::SubscriptionIdExhausted => {
                (StatusCode::INTERNAL_SERVER_ERROR, "capacity_exhausted")
            }
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
            CoreError::SchemaViolation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "schema_violation"),
            CoreError::SchemaRegistry(_) => (StatusCode::CONFLICT, "schema_overlap"),
            CoreError::ExistingSchemaViolations { .. } => {
                (StatusCode::CONFLICT, "existing_schema_violations")
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
        #[serde(default)]
        expiry: Option<WireExpiry>,
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
        #[serde(default)]
        expiry: Option<WireExpiry>,
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
                expiry: expiry_update(expiry)?,
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
                expiry: expiry_update(expiry)?,
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

fn expiry_update(value: Option<WireExpiry>) -> Result<ExpiryUpdate, ApiError> {
    value
        .map(TryInto::try_into)
        .transpose()
        .map(|expiry| expiry.unwrap_or(ExpiryUpdate::Preserve))
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
        Diagnostic::SchemaWarning(issue) => {
            json!({"code": "schema_warning", "topic": issue.topic(), "message": issue.kind().to_string()})
        }
    }
}
