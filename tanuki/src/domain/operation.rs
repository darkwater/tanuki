use thiserror::Error;

use super::{InputDefinition, NonNegativeDuration, TopicPath, Value};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputKind {
    Desired,
    Command,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExpiryUpdate {
    Preserve,
    Set(NonNegativeDuration),
    Clear,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClaimRelease {
    Immediate,
    After(NonNegativeDuration),
}

#[derive(Clone, Debug, PartialEq)]
pub enum WriteOperation {
    PublishState {
        topic: TopicPath,
        value: Value,
        expiry: ExpiryUpdate,
    },
    PublishEvent {
        topic: TopicPath,
        value: Value,
    },
    DefineInput {
        topic: TopicPath,
        kind: InputKind,
        definition: InputDefinition,
    },
    ClaimInput {
        topic: TopicPath,
        release: ClaimRelease,
    },
    SubmitDesired {
        topic: TopicPath,
        value: Value,
        expiry: ExpiryUpdate,
    },
    SubmitCommand {
        topic: TopicPath,
        value: Value,
    },
    ClearDesired {
        topic: TopicPath,
    },
    RemoveNode {
        topic: TopicPath,
    },
}

impl WriteOperation {
    #[must_use]
    pub fn topic(&self) -> &TopicPath {
        match self {
            Self::PublishState { topic, .. }
            | Self::PublishEvent { topic, .. }
            | Self::DefineInput { topic, .. }
            | Self::ClaimInput { topic, .. }
            | Self::SubmitDesired { topic, .. }
            | Self::SubmitCommand { topic, .. }
            | Self::ClearDesired { topic }
            | Self::RemoveNode { topic } => topic,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WriteBatch {
    operations: Vec<WriteOperation>,
}

impl WriteBatch {
    pub fn new(operations: Vec<WriteOperation>) -> Result<Self, BatchError> {
        if operations.is_empty() {
            Err(BatchError::Empty)
        } else {
            Ok(Self { operations })
        }
    }

    pub fn operations(&self) -> &[WriteOperation] {
        &self.operations
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BatchError {
    #[error("a write batch must contain at least one operation")]
    Empty,
}
