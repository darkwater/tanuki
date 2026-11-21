use std::{convert::Infallible, time::Duration};

use axum::{
    Extension, Json, Router,
    extract::{
        FromRequestParts, Path, Query, State, WebSocketUpgrade,
        rejection::{JsonRejection, QueryRejection},
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{StatusCode, request::Parts},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post, put},
};
use futures_util::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};

use thiserror::Error;
use tracing::warn;

use crate::{
    core::{CoreError, SchemaInstallMode, Subscription, SubscriptionCapacity, SubscriptionEnd},
    domain::{
        ClientName, FiniteF64, NodeKind, Selection, Selector, TopicPath, ValueKind, WriteBatch,
        WriteContext, WriteOperation,
    },
    link::{LinkDefinition, LinkName},
    protocol::{
        ClientMessage, CommitView, ErrorView, JsonValue, LinkInstallView, LinkRemovalView,
        RequestConversionError, RequestId, SchemaInstallView, ServerMessage, SessionMode,
        SnapshotView, UpdateView, WireExpiry, WireOperation, expiry_update, nonnegative_duration,
    },
    runtime::{OpenedConnection, RuntimeError, RuntimeHandle, StreamLease},
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueCast, ValueValidator},
};

pub use crate::runtime::{Clock, SharedCore};
type HttpState = RuntimeHandle;

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const SUBSCRIPTION_BATCH_CAPACITY: usize = 64;

/// WebSocket liveness bounds. `None` disables a bound. The defaults are
/// provisional; see `docs/protocol.md`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketTimeouts {
    /// Time allowed between the upgrade and the first (hello) message.
    pub hello: Option<Duration>,
    /// Server pings and the silence deadline they make meaningful.
    pub heartbeat: Option<Heartbeat>,
    /// Bound on each outgoing write, so a stalled peer cannot hold its session.
    pub send: Option<Duration>,
}

/// The server pings every `interval`. A peer that sends nothing at all (data,
/// ping or pong) for `silence` is closed, running ordinary disconnect cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Heartbeat {
    pub interval: Duration,
    pub silence: Duration,
}

impl WebSocketTimeouts {
    /// No liveness bounds. Paused-clock tests need this: Tokio auto-advances
    /// virtual time to the next timer during real socket I/O, however distant.
    pub const DISABLED: Self = Self {
        hello: None,
        heartbeat: None,
        send: None,
    };
}

impl Default for WebSocketTimeouts {
    fn default() -> Self {
        Self {
            hello: Some(Duration::from_secs(10)),
            heartbeat: Some(Heartbeat {
                interval: Duration::from_secs(20),
                silence: Duration::from_secs(60),
            }),
            send: Some(Duration::from_secs(10)),
        }
    }
}

/// Runs `future` within `limit`, or unbounded when `limit` is `None`.
/// Returns `None` if the limit elapsed first.
async fn within<F: Future>(limit: Option<Duration>, future: F) -> Option<F::Output> {
    match limit {
        Some(limit) => tokio::time::timeout(limit, future).await.ok(),
        None => Some(future.await),
    }
}

/// Sleeps until `deadline`, or forever when there is none.
async fn sleep_until_or_never(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

pub fn router(runtime: RuntimeHandle) -> Router {
    router_with_timeouts(runtime, WebSocketTimeouts::default())
}

pub fn router_with_timeouts(runtime: RuntimeHandle, timeouts: WebSocketTimeouts) -> Router {
    Router::new()
        .route("/v1/write", post(batch_write))
        .route("/v1/state/{*topic}", post(single_state_write))
        .route("/v1/links/{name}", put(install_link).delete(remove_link))
        .route("/v1/schemas/{name}", put(install_schema))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/sse", get(sse))
        .route("/v1/ws", get(websocket_upgrade))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(runtime)
        .layer(Extension(timeouts))
}

#[derive(Deserialize)]
struct LinkRequest {
    mount: TopicPath,
    target: TopicPath,
}

async fn install_link(
    State(state): State<HttpState>,
    Path(name): Path<String>,
    StatelessActor(actor): StatelessActor,
    payload: Result<Json<LinkRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<LinkInstallView>>, ApiError> {
    let Json(request) = payload.map_err(ApiError::invalid_json)?;
    let name = LinkName::parse(&name)
        .map_err(|error| ApiError::bad_request("invalid_link", error.to_string()))?;
    let definition = LinkDefinition::new(name.clone(), request.mount, request.target)
        .map_err(|error| ApiError::bad_request("invalid_link", error.to_string()))?;
    let outcome = state
        .install_link(definition)
        .map_err(ApiError::from_runtime)?;
    for warning in outcome.warnings() {
        warn!(?warning, link = %name, client = %actor.client(), "link installed with diagnostic");
    }
    Ok(Json(ApiResponse::success(LinkInstallView::new(
        &name, &outcome,
    ))))
}

async fn remove_link(
    State(state): State<HttpState>,
    Path(name): Path<String>,
    StatelessActor(actor): StatelessActor,
) -> Result<Json<ApiResponse<LinkRemovalView>>, ApiError> {
    let name = LinkName::parse(&name)
        .map_err(|error| ApiError::bad_request("invalid_link", error.to_string()))?;
    let outcome = state.remove_link(&name).map_err(ApiError::from_runtime)?;
    warn!(link = %name, client = %actor.client(), removed = outcome.removed(), "link removal requested");
    Ok(Json(ApiResponse::success(LinkRemovalView::new(
        &name, &outcome,
    ))))
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
    expected_update_interval: Option<String>,
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
    StatelessActor(actor): StatelessActor,
    payload: Result<Json<SchemaRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SchemaInstallView>>, ApiError> {
    let Json(request) = payload.map_err(ApiError::invalid_json)?;
    let schema = build_schema(&name, request.rules)?;
    let mode = if request.force {
        SchemaInstallMode::RemoveInvalid
    } else {
        SchemaInstallMode::RejectInvalid
    };
    let outcome = state
        .install_schema(schema, mode)
        .map_err(ApiError::from_runtime)?;
    for warning in outcome.warnings() {
        warn!(?warning, schema = %name, client = %actor.client(), "schema installed with diagnostic");
    }
    Ok(Json(ApiResponse::success(SchemaInstallView::new(
        name, &outcome,
    ))))
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
    let rule = match request.node_kind {
        Some(node_kind) => rule.with_node_kind(node_kind.into()),
        None => rule,
    };
    Ok(match request.expected_update_interval {
        Some(interval) => rule
            .with_expected_update_interval(
                nonnegative_duration(&interval).map_err(ApiError::from_request_conversion)?,
            )
            .map_err(|error| ApiError::bad_request("invalid_schema", error.to_string()))?,
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

async fn websocket_upgrade(
    State(state): State<HttpState>,
    Extension(timeouts): Extension<WebSocketTimeouts>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let lease = state.track_stream().map_err(ApiError::from_runtime)?;
    Ok(upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| websocket_session(socket, state, lease, timeouts)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WireCodec {
    Json,
    MessagePack,
}

async fn websocket_session(
    mut socket: WebSocket,
    state: HttpState,
    _lease: StreamLease,
    timeouts: WebSocketTimeouts,
) {
    let mut stopped = state.shutdown_signal();
    tokio::select! {
        biased;
        _ = async { let _ = stopped.wait_for(|stopped| *stopped).await; } => {
            // Close delivery is bounded; shutdown never waits for a peer response.
            close_socket(&mut socket, Some(Duration::from_secs(1)), 1001, "server_shutdown").await;
        }
        () = run_websocket_session(&mut socket, &state, timeouts) => {}
    }
}

/// Best-effort close frame, bounded so a stalled peer cannot delay cleanup.
async fn close_socket(
    socket: &mut WebSocket,
    limit: Option<Duration>,
    code: u16,
    reason: &'static str,
) {
    let close = Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }));
    let _ = within(limit, socket.send(close)).await;
}

async fn run_websocket_session(
    socket: &mut WebSocket,
    state: &HttpState,
    timeouts: WebSocketTimeouts,
) {
    let first = match within(timeouts.hello, socket.recv()).await {
        Some(Some(Ok(first))) => first,
        Some(None | Some(Err(_))) => return,
        None => {
            warn!(limit = ?timeouts.hello, "WebSocket peer sent no hello in time; closing");
            close_socket(socket, timeouts.send, 1008, "hello_timeout").await;
            return;
        }
    };
    let (codec, first) = match decode_client_message(first) {
        Ok(decoded) => decoded,
        Err((codec, error)) => {
            let _ = send_server_message(
                socket,
                timeouts.send,
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
        session,
    } = first
    else {
        let _ = send_server_message(
            socket,
            timeouts.send,
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
    let selection = Selection::new(selectors);
    match session {
        SessionMode::Stateless => {
            let mut subscription = match state.subscribe(selection, subscription_capacity()) {
                Ok(subscription) => subscription,
                Err(error) => {
                    let error = api_server_error(Some(request_id), ApiError::from_runtime(error));
                    let _ = send_server_message(socket, timeouts.send, codec, &error).await;
                    return;
                }
            };
            let snapshot =
                crate::protocol::snapshot_message(request_id, subscription.snapshot(), &[]);
            if send_server_message(socket, timeouts.send, codec, &snapshot)
                .await
                .is_ok()
            {
                let context = WriteContext::stateless(client);
                websocket_loop(socket, state, codec, &context, &mut subscription, timeouts).await;
            }
        }
        SessionMode::Managed => {
            run_managed_session(
                socket, state, timeouts, codec, request_id, client, selection,
            )
            .await;
        }
    }
}

fn subscription_capacity() -> SubscriptionCapacity {
    SubscriptionCapacity::new(SUBSCRIPTION_BATCH_CAPACITY)
        .expect("configured subscription capacity is nonzero")
}

async fn run_managed_session(
    socket: &mut WebSocket,
    state: &HttpState,
    timeouts: WebSocketTimeouts,
    codec: WireCodec,
    request_id: RequestId,
    client: ClientName,
    selection: Selection,
) {
    let opened = match state.open_connection(client, selection, subscription_capacity()) {
        Ok(value) => value,
        Err(api) => {
            let _ = send_server_message(
                socket,
                timeouts.send,
                codec,
                &api_server_error(Some(request_id), ApiError::from_runtime(api)),
            )
            .await;
            return;
        }
    };
    let OpenedConnection {
        opened,
        mut subscription,
        mut kicked,
        session,
    } = opened;
    let handle = session.handle();
    for warning in opened.warnings() {
        warn!(?warning, client = %handle.client(), "WebSocket session opened with diagnostic");
    }
    for release in opened.pending_releases() {
        warn!(topic = %release.topic(), claim = release.claim().get(), "replacement claim release scheduled");
    }

    let snapshot =
        crate::protocol::snapshot_message(request_id, subscription.snapshot(), opened.warnings());
    let context = WriteContext::managed(handle.clone());
    tokio::select! {
        biased;
        _ = async { let _ = kicked.wait_for(|kicked| *kicked).await; } => {
            close_socket(socket, Some(Duration::from_secs(1)), 4001, "session_replaced").await;
        }
        () = async {
            if send_server_message(socket, timeouts.send, codec, &snapshot).await.is_ok() {
                websocket_loop(socket, state, codec, &context, &mut subscription, timeouts).await;
            }
        } => {}
    }
}

async fn websocket_loop(
    socket: &mut WebSocket,
    state: &HttpState,
    subscription_codec: WireCodec,
    context: &WriteContext,
    subscription: &mut Subscription,
    timeouts: WebSocketTimeouts,
) {
    // Any frame from the peer proves liveness; pings give a quiet but healthy
    // peer something to answer. Tungstenite answers peer pings by itself.
    let heartbeat = timeouts.heartbeat;
    let mut last_heard = tokio::time::Instant::now();
    let mut next_ping = heartbeat.map(|heartbeat| last_heard + heartbeat.interval);
    loop {
        let silence_deadline = heartbeat.map(|heartbeat| last_heard + heartbeat.silence);
        tokio::select! {
            () = sleep_until_or_never(silence_deadline) => {
                warn!(client = %context.client(), ?heartbeat, "WebSocket peer stopped responding; closing session");
                close_socket(socket, timeouts.send, 1011, "peer_timeout").await;
                return;
            }
            () = sleep_until_or_never(next_ping) => {
                next_ping = heartbeat.map(|heartbeat| tokio::time::Instant::now() + heartbeat.interval);
                if send_frame(socket, timeouts.send, Message::Ping(Default::default())).await.is_err() {
                    return;
                }
            }
            update = subscription.update() => {
                let Some(update) = update else {
                    if subscription.end_reason() == Some(SubscriptionEnd::SlowConsumer) {
                        close_socket(socket, timeouts.send, 1013, "slow_consumer").await;
                    }
                    return;
                };
                if send_server_message(socket, timeouts.send, subscription_codec, &crate::protocol::update_message(&update)).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(incoming)) = incoming else { return };
                last_heard = tokio::time::Instant::now();
                let (incoming_codec, message) = match incoming {
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(_) => return,
                    Message::Text(_) | Message::Binary(_) => match decode_client_message(incoming) {
                        Ok(decoded) => decoded,
                        Err((incoming_codec, error)) => {
                            let error = error_message(None, "invalid_message", error);
                            if send_server_message(socket, timeouts.send, incoming_codec, &error).await.is_err() { return; }
                            continue;
                        }
                    },
                };
                let response = match message {
                    ClientMessage::Hello { request_id, .. } => error_message(
                        Some(request_id),
                        "already_initialized",
                        "hello is only valid as the first message",
                    ),
                    ClientMessage::Write { request_id, operations } => {
                        let response = apply_websocket_write(state, context, request_id, operations);
                        // The commit's own selected update (and anything queued
                        // before it) goes out ahead of the reply, so a client's
                        // cache already reflects its write when the reply lands.
                        while let Some(update) = subscription.try_update() {
                            if send_server_message(socket, timeouts.send, subscription_codec, &crate::protocol::update_message(&update)).await.is_err() { return; }
                        }
                        response
                    }
                    ClientMessage::Select { request_id, selectors } => {
                        // Queued updates of the old selection are superseded by
                        // the new snapshot, which is taken after all of them.
                        match state.subscribe(Selection::new(selectors), subscription_capacity()) {
                            Ok(next) => {
                                *subscription = next;
                                crate::protocol::snapshot_message(request_id, subscription.snapshot(), &[])
                            }
                            Err(error) => api_server_error(Some(request_id), ApiError::from_runtime(error)),
                        }
                    }
                };
                let expired = matches!(&response, ServerMessage::Error { error, .. } if error.code == "session_expired");
                if send_server_message(socket, timeouts.send, incoming_codec, &response).await.is_err() { return; }
                if expired { return; }
            }
        }
    }
}

fn apply_websocket_write(
    state: &HttpState,
    context: &WriteContext,
    request_id: RequestId,
    operations: Vec<WireOperation>,
) -> ServerMessage {
    let batch = match WriteBatch::try_from(operations) {
        Ok(batch) => batch,
        Err(error) => {
            return api_server_error(Some(request_id), ApiError::from_request_conversion(error));
        }
    };
    match state.apply(context, batch).map_err(ApiError::from_runtime) {
        Ok(outcome) => {
            for diagnostic in outcome.warnings() {
                warn!(?diagnostic, client = %context.client(), "WebSocket write accepted with diagnostic");
            }
            ServerMessage::Reply {
                request_id,
                result: serde_json::to_value(CommitView::from(&outcome))
                    .expect("commit replies always serialize"),
            }
        }
        Err(error) => api_server_error(Some(request_id), error),
    }
}

fn decode_client_message(
    message: Message,
) -> Result<(WireCodec, ClientMessage), (WireCodec, String)> {
    match message {
        Message::Text(text) => serde_json::from_str(&text)
            .map(|message| (WireCodec::Json, message))
            .map_err(|error| (WireCodec::Json, error.to_string())),
        Message::Binary(bytes) => crate::protocol::decode_messagepack(&bytes)
            .map(|message| (WireCodec::MessagePack, message))
            .map_err(|error| (WireCodec::MessagePack, error.to_string())),
        _ => Err((
            WireCodec::Json,
            "expected a text or binary data message".to_owned(),
        )),
    }
}

/// Why an outgoing message did not reach the socket. Encoding, size and
/// timeout failures are logged where detected; transport errors are ordinary
/// disconnects. Callers only need to stop the session.
#[derive(Debug, Error)]
enum SendFailure {
    #[error("JSON server message encoding failed: {0}")]
    EncodeJson(#[source] serde_json::Error),
    #[error("MessagePack server message encoding failed: {0}")]
    EncodeMessagePack(#[source] rmp_serde::encode::Error),
    #[error("encoded server message is {bytes} bytes, above the {MAX_MESSAGE_BYTES}-byte limit")]
    TooLarge { bytes: usize },
    #[error("WebSocket write did not complete within {0:?}")]
    Timeout(Duration),
    #[error("WebSocket write failed: {0}")]
    Transport(#[source] axum::Error),
}

async fn send_server_message(
    socket: &mut WebSocket,
    limit: Option<Duration>,
    codec: WireCodec,
    message: &ServerMessage,
) -> Result<(), SendFailure> {
    let encoded = match codec {
        WireCodec::Json => serde_json::to_string(message)
            .map(|text| (text.len(), Message::Text(text.into())))
            .map_err(SendFailure::EncodeJson),
        WireCodec::MessagePack => rmp_serde::to_vec_named(message)
            .map(|bytes| (bytes.len(), Message::Binary(bytes.into())))
            .map_err(SendFailure::EncodeMessagePack),
    };
    let frame = match encoded {
        Ok((bytes, _)) if bytes > MAX_MESSAGE_BYTES => {
            // Close visibly rather than vanish: the peer sees 1009 and the
            // operator sees which message kind overflowed.
            let failure = SendFailure::TooLarge { bytes };
            warn!(%failure, kind = server_message_kind(message), "closing WebSocket: message too big to deliver");
            close_socket(socket, limit, 1009, "message_too_big").await;
            return Err(failure);
        }
        Ok((_, frame)) => frame,
        Err(failure) => {
            warn!(%failure, "closing WebSocket: server message could not be encoded");
            return Err(failure);
        }
    };
    send_frame(socket, limit, frame).await
}

async fn send_frame(
    socket: &mut WebSocket,
    limit: Option<Duration>,
    frame: Message,
) -> Result<(), SendFailure> {
    match within(limit, socket.send(frame)).await {
        Some(Ok(())) => Ok(()),
        Some(Err(source)) => Err(SendFailure::Transport(source)),
        None => {
            let failure = SendFailure::Timeout(limit.unwrap_or_default());
            warn!(%failure, "closing WebSocket: peer is not accepting data");
            Err(failure)
        }
    }
}

const fn server_message_kind(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::Snapshot { .. } => "snapshot",
        ServerMessage::Update { .. } => "update",
        ServerMessage::Reply { .. } => "reply",
        ServerMessage::Error { .. } => "error",
    }
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
) -> Result<Json<ApiResponse<CommitView>>, ApiError> {
    let Json(payload) = payload.map_err(ApiError::invalid_json)?;
    let batch =
        WriteBatch::try_from(payload.operations).map_err(ApiError::from_request_conversion)?;
    apply(&state, actor, batch)
}

async fn single_state_write(
    State(state): State<HttpState>,
    StatelessActor(actor): StatelessActor,
    Path(topic): Path<String>,
    payload: Result<Json<StateRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<CommitView>>, ApiError> {
    let Json(payload) = payload.map_err(ApiError::invalid_json)?;
    let topic = TopicPath::parse(&format!("/{topic}"))
        .map_err(|error| ApiError::bad_request("invalid_topic", error.to_string()))?;
    apply(
        &state,
        actor,
        WriteBatch::new(vec![WriteOperation::PublishState {
            topic,
            value: payload.value.into_inner(),
            expiry: expiry_update(payload.expiry).map_err(ApiError::from_request_conversion)?,
        }])
        .map_err(|error| ApiError::bad_request("empty_batch", error.to_string()))?,
    )
}

fn apply(
    state: &HttpState,
    actor: WriteContext,
    batch: WriteBatch,
) -> Result<Json<ApiResponse<CommitView>>, ApiError> {
    let outcome = state.apply(&actor, batch).map_err(ApiError::from_runtime)?;
    for diagnostic in outcome.warnings() {
        warn!(?diagnostic, client = %actor.client(), "write accepted with diagnostic");
    }
    Ok(Json(ApiResponse::success(CommitView::from(&outcome))))
}

async fn snapshot(
    State(state): State<HttpState>,
    query: Result<Query<ReadQuery>, QueryRejection>,
) -> Result<Json<ApiResponse<SnapshotView>>, ApiError> {
    let Query(query) =
        query.map_err(|error| ApiError::bad_request("invalid_query", error.body_text()))?;
    let snapshot = state
        .read(&Selection::new(vec![query.select]))
        .map_err(ApiError::from_runtime)?;
    Ok(Json(ApiResponse::success(SnapshotView::from(&snapshot))))
}

async fn sse(
    State(state): State<HttpState>,
    query: Result<Query<ReadQuery>, QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let Query(query) =
        query.map_err(|error| ApiError::bad_request("invalid_query", error.body_text()))?;
    let lease = state.track_stream().map_err(ApiError::from_runtime)?;
    let stopped = state.shutdown_signal();
    let subscription = state
        .subscribe(
            Selection::new(vec![query.select]),
            SubscriptionCapacity::new(SUBSCRIPTION_BATCH_CAPACITY)
                .expect("configured subscription capacity is nonzero"),
        )
        .map_err(ApiError::from_runtime)?;
    let initial = sse_json_event(
        "snapshot",
        subscription.snapshot().sequence().get(),
        &SnapshotView::from(subscription.snapshot()),
    );
    let updates = stream::unfold(
        (subscription, stopped, lease),
        |(mut subscription, mut stopped, lease)| async move {
            let update = tokio::select! {
                biased;
                _ = async { let _ = stopped.wait_for(|stopped| *stopped).await; } => return None,
                update = subscription.update() => update,
            };
            match update {
                Some(update) => {
                    let event = sse_json_event(
                        "update",
                        update.sequence().get(),
                        &UpdateView::from(&update),
                    );
                    Some((Ok(event), (subscription, stopped, lease)))
                }
                None => {
                    if subscription.end_reason() == Some(SubscriptionEnd::SlowConsumer) {
                        warn!("SSE subscription disconnected as a slow consumer");
                    }
                    None
                }
            }
        },
    );
    let events = stream::once(async { Ok(initial) }).chain(updates);
    Ok(Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

fn sse_json_event<T: Serialize>(name: &'static str, sequence: u64, value: &T) -> Event {
    let data = serde_json::to_string(value).expect("protocol views always serialize to JSON");
    Event::default()
        .event(name)
        .id(sequence.to_string())
        .data(data)
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

#[derive(Serialize)]
struct ApiErrorResponse {
    ok: bool,
    error: ErrorView,
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

    fn from_request_conversion(error: RequestConversionError) -> Self {
        let code = match &error {
            RequestConversionError::InvalidDuration(_) => "invalid_duration",
            RequestConversionError::NegativeDuration(_) => "negative_duration",
            RequestConversionError::Batch(_) => "empty_batch",
        };
        Self::bad_request(code, error.to_string())
    }

    fn invalid_json(error: JsonRejection) -> Self {
        Self::bad_request("invalid_json", error.body_text())
    }

    fn from_runtime(error: RuntimeError) -> Self {
        match error {
            RuntimeError::Core(error) => Self::from_core(error),
            RuntimeError::Stopping => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "server_shutdown",
                message: "server is shutting down".to_owned(),
            },
            RuntimeError::Poisoned => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                message: "server state lock was poisoned".to_owned(),
            },
        }
    }

    fn from_core(error: CoreError) -> Self {
        let (status, code) = match &error {
            CoreError::SystemTopic { .. } => (StatusCode::FORBIDDEN, "system_topic"),
            CoreError::ValueTooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "value_too_large"),
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
            CoreError::Link(_) => (StatusCode::CONFLICT, "invalid_link"),
        };
        let message = match &error {
            CoreError::ExistingSchemaViolations { violations } => violations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
            _ => error.to_string(),
        };
        Self {
            status,
            code,
            message,
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
            Json(ApiErrorResponse {
                ok: false,
                error: ErrorView {
                    code: self.code.to_owned(),
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}
