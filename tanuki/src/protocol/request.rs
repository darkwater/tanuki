//! Validated wire requests become domain operations without HTTP dependencies.
use super::{WireExpiry, WireInputKind, WireOperation, WireRelease};
use crate::domain::{
    ClaimRelease, ExpiryUpdate, InputDefinition, InputKind, NonNegativeDuration, WriteBatch,
    WriteOperation,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RequestConversionError {
    #[error(transparent)]
    InvalidDuration(jiff::Error),
    #[error(transparent)]
    NegativeDuration(tanuki_protocol::NegativeDuration),
    #[error(transparent)]
    Batch(#[from] crate::domain::BatchError),
}

impl TryFrom<Vec<WireOperation>> for WriteBatch {
    type Error = RequestConversionError;
    fn try_from(operations: Vec<WireOperation>) -> Result<Self, Self::Error> {
        let operations = operations
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(operations)?)
    }
}

impl TryFrom<WireOperation> for WriteOperation {
    type Error = RequestConversionError;

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

impl From<WireInputKind> for InputKind {
    fn from(value: WireInputKind) -> Self {
        match value {
            WireInputKind::Desired => Self::Desired,
            WireInputKind::Command => Self::Command,
        }
    }
}

impl TryFrom<WireExpiry> for ExpiryUpdate {
    type Error = RequestConversionError;

    fn try_from(value: WireExpiry) -> Result<Self, Self::Error> {
        Ok(match value {
            WireExpiry::Preserve => Self::Preserve,
            WireExpiry::Clear => Self::Clear,
            WireExpiry::Set { duration } => Self::Set(nonnegative_duration(&duration)?),
        })
    }
}

pub fn expiry_update(value: Option<WireExpiry>) -> Result<ExpiryUpdate, RequestConversionError> {
    value
        .map(TryInto::try_into)
        .transpose()
        .map(|expiry| expiry.unwrap_or(ExpiryUpdate::Preserve))
}

impl TryFrom<WireRelease> for ClaimRelease {
    type Error = RequestConversionError;

    fn try_from(value: WireRelease) -> Result<Self, Self::Error> {
        Ok(match value {
            WireRelease::Immediate => Self::Immediate,
            WireRelease::After { duration } => Self::After(nonnegative_duration(&duration)?),
        })
    }
}

pub fn nonnegative_duration(input: &str) -> Result<NonNegativeDuration, RequestConversionError> {
    let duration = input
        .parse()
        .map_err(RequestConversionError::InvalidDuration)?;
    NonNegativeDuration::new(duration).map_err(RequestConversionError::NegativeDuration)
}
