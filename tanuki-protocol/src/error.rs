use std::fmt;

use serde::{Deserialize, Serialize};

/// A stable machine-readable error code, shared by HTTP error bodies and
/// WebSocket `error` messages. Codes added by a newer server decode as
/// [`ErrorCode::Unknown`]; the accompanying message still explains them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// A second `hello` on an open WebSocket.
    AlreadyInitialized,
    /// An internal identifier or sequence space is exhausted.
    CapacityExhausted,
    /// The input definition is controlled by another session's claim.
    ClaimAuthorityRequired,
    /// The `tanuki-client` header and `client` query parameter disagree.
    ConflictingClient,
    /// An expiry or grace deadline is outside the supported timestamp range.
    DeadlineOutOfRange,
    /// A write batch has no operations.
    EmptyBatch,
    /// Existing values violate a schema being installed without `force`.
    ExistingSchemaViolations,
    /// The first WebSocket message was not `hello`.
    HelloRequired,
    /// A submission, claim or clear targets an input that is not defined.
    InputNotDefined,
    InternalError,
    InvalidClient,
    InvalidDuration,
    InvalidJson,
    InvalidLink,
    /// A WebSocket message could not be decoded.
    InvalidMessage,
    InvalidQuery,
    InvalidSchema,
    InvalidTopic,
    MethodNotAllowed,
    /// An HTTP write carries no client name.
    MissingClient,
    NegativeDuration,
    NotFound,
    /// A schema rule overlaps a rule of another installed schema.
    SchemaOverlap,
    /// A denying schema rejected the batch.
    SchemaViolation,
    ServerShutdown,
    /// The managed session was replaced or disconnected.
    SessionExpired,
    /// Claiming needs a managed session; stateless writers cannot claim.
    SessionRequired,
    /// Writes to the reserved `/$...` namespace are not permitted.
    SystemTopic,
    /// A value's JSON encoding exceeds the per-value limit.
    ValueTooLarge,
    /// The operation does not fit the existing node's kind.
    WrongNodeKind,
    /// A code this version does not know.
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyInitialized => "already_initialized",
            Self::CapacityExhausted => "capacity_exhausted",
            Self::ClaimAuthorityRequired => "claim_authority_required",
            Self::ConflictingClient => "conflicting_client",
            Self::DeadlineOutOfRange => "deadline_out_of_range",
            Self::EmptyBatch => "empty_batch",
            Self::ExistingSchemaViolations => "existing_schema_violations",
            Self::HelloRequired => "hello_required",
            Self::InputNotDefined => "input_not_defined",
            Self::InternalError => "internal_error",
            Self::InvalidClient => "invalid_client",
            Self::InvalidDuration => "invalid_duration",
            Self::InvalidJson => "invalid_json",
            Self::InvalidLink => "invalid_link",
            Self::InvalidMessage => "invalid_message",
            Self::InvalidQuery => "invalid_query",
            Self::InvalidSchema => "invalid_schema",
            Self::InvalidTopic => "invalid_topic",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::MissingClient => "missing_client",
            Self::NegativeDuration => "negative_duration",
            Self::NotFound => "not_found",
            Self::SchemaOverlap => "schema_overlap",
            Self::SchemaViolation => "schema_violation",
            Self::ServerShutdown => "server_shutdown",
            Self::SessionExpired => "session_expired",
            Self::SessionRequired => "session_required",
            Self::SystemTopic => "system_topic",
            Self::ValueTooLarge => "value_too_large",
            Self::WrongNodeKind => "wrong_node_kind",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
