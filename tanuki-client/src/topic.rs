use crate::{
    ClientError, PayloadError, Session, WriteReceipt, from_value,
    protocol::*,
    session::{Inner, write},
    to_value,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    marker::PhantomData,
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    State,
    Event,
    Desired,
    Command,
}
mod sealed {
    pub trait Sealed {}
}
pub trait TopicKind: sealed::Sealed {
    const KIND: NodeKind;
}
pub trait InputTopicKind: TopicKind {
    const INPUT: WireInputKind;
}
pub enum State {}
pub enum Event {}
pub enum Desired {}
pub enum Command {}
impl sealed::Sealed for State {}
impl sealed::Sealed for Event {}
impl sealed::Sealed for Desired {}
impl sealed::Sealed for Command {}
impl TopicKind for State {
    const KIND: NodeKind = NodeKind::State;
}
impl TopicKind for Event {
    const KIND: NodeKind = NodeKind::Event;
}
impl TopicKind for Desired {
    const KIND: NodeKind = NodeKind::Desired;
}
impl TopicKind for Command {
    const KIND: NodeKind = NodeKind::Command;
}
impl InputTopicKind for Desired {
    const INPUT: WireInputKind = WireInputKind::Desired;
}
impl InputTopicKind for Command {
    const INPUT: WireInputKind = WireInputKind::Command;
}

pub struct Topic<T, K: TopicKind> {
    path: TopicPath,
    session: Weak<Inner>,
    marker: PhantomData<fn() -> (T, K)>,
}
impl<T, K: TopicKind> Clone for Topic<T, K> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            session: self.session.clone(),
            marker: PhantomData,
        }
    }
}
pub type StateTopic<T> = Topic<T, State>;
pub type EventTopic<T> = Topic<T, Event>;
pub type DesiredTopic<T> = Topic<T, Desired>;
pub type CommandTopic<T> = Topic<T, Command>;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpiryUpdate {
    Preserve,
    Clear,
    Set(NonNegativeDuration),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimRelease {
    Immediate,
    After(NonNegativeDuration),
}
impl From<ExpiryUpdate> for WireExpiry {
    fn from(value: ExpiryUpdate) -> Self {
        match value {
            ExpiryUpdate::Preserve => Self::Preserve,
            ExpiryUpdate::Clear => Self::Clear,
            ExpiryUpdate::Set(d) => Self::Set {
                duration: jiff::fmt::temporal::SpanPrinter::new().duration_to_string(&d.get()),
            },
        }
    }
}
impl From<ClaimRelease> for WireRelease {
    fn from(value: ClaimRelease) -> Self {
        match value {
            ClaimRelease::Immediate => Self::Immediate,
            ClaimRelease::After(d) => Self::After {
                duration: jiff::fmt::temporal::SpanPrinter::new().duration_to_string(&d.get()),
            },
        }
    }
}
impl Session {
    fn topic<T, K: TopicKind>(&self, path: &str) -> Result<Topic<T, K>, ClientError> {
        Ok(Topic {
            path: TopicPath::parse(path)?,
            session: Arc::downgrade(&self.inner),
            marker: PhantomData,
        })
    }
    pub fn state<T>(&self, path: &str) -> Result<StateTopic<T>, ClientError> {
        self.topic(path)
    }
    pub fn event<T>(&self, path: &str) -> Result<EventTopic<T>, ClientError> {
        self.topic(path)
    }
    pub fn desired<T>(&self, path: &str) -> Result<DesiredTopic<T>, ClientError> {
        self.topic(path)
    }
    pub fn command<T>(&self, path: &str) -> Result<CommandTopic<T>, ClientError> {
        self.topic(path)
    }
    pub fn batch(&self) -> Batch {
        Batch {
            session: Arc::downgrade(&self.inner),
            operations: vec![],
        }
    }
}
impl<T, K: TopicKind> Topic<T, K> {
    pub(crate) fn check_session(&self, session: &Session) -> Result<(), ClientError> {
        if self.session.ptr_eq(&Arc::downgrade(&session.inner)) {
            Ok(())
        } else {
            Err(ClientError::WrongSession)
        }
    }
    pub fn path(&self) -> &TopicPath {
        &self.path
    }
    pub async fn remove(&self) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::RemoveNode {
                topic: self.path.clone(),
            }],
        )
        .await
    }
    pub fn decode(&self, node: &NodeView) -> Result<DecodedNode<T>, ReadError>
    where
        T: DeserializeOwned,
    {
        let actual = node_kind(node);
        if actual != K::KIND {
            return Err(ReadError::WrongKind {
                topic: self.path.clone(),
                expected: K::KIND,
                actual,
            });
        }
        decode_node(node).map_err(|source| ReadError::Payload {
            topic: self.path.clone(),
            source,
        })
    }
}
impl<T> StateTopic<T> {
    pub async fn publish(
        &self,
        value: &T,
        expiry: ExpiryUpdate,
    ) -> Result<WriteReceipt, ClientError>
    where
        T: Serialize,
    {
        self.publish_value(to_value(value)?, expiry).await
    }
    pub async fn publish_value(
        &self,
        value: Value,
        expiry: ExpiryUpdate,
    ) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::PublishState {
                topic: self.path.clone(),
                value: JsonValue::new(value),
                expiry: Some(expiry.into()),
            }],
        )
        .await
    }
}
impl<T> EventTopic<T> {
    pub async fn emit(&self, value: &T) -> Result<WriteReceipt, ClientError>
    where
        T: Serialize,
    {
        self.emit_value(to_value(value)?).await
    }
    pub async fn emit_value(&self, value: Value) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::PublishEvent {
                topic: self.path.clone(),
                value: JsonValue::new(value),
            }],
        )
        .await
    }
}
impl<T, K: InputTopicKind> Topic<T, K> {
    pub async fn define(&self) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::DefineInput {
                topic: self.path.clone(),
                kind: K::INPUT,
            }],
        )
        .await
    }
    pub async fn claim(&self, release: ClaimRelease) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::ClaimInput {
                topic: self.path.clone(),
                release: release.into(),
            }],
        )
        .await
    }
}
impl<T> DesiredTopic<T> {
    pub async fn submit(&self, value: &T, expiry: ExpiryUpdate) -> Result<WriteReceipt, ClientError>
    where
        T: Serialize,
    {
        self.submit_value(to_value(value)?, expiry).await
    }
    pub async fn submit_value(
        &self,
        value: Value,
        expiry: ExpiryUpdate,
    ) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::SubmitDesired {
                topic: self.path.clone(),
                value: JsonValue::new(value),
                expiry: Some(expiry.into()),
            }],
        )
        .await
    }
    pub async fn clear(&self) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::ClearDesired {
                topic: self.path.clone(),
            }],
        )
        .await
    }
}
impl<T> CommandTopic<T> {
    pub async fn submit(&self, value: &T) -> Result<WriteReceipt, ClientError>
    where
        T: Serialize,
    {
        self.submit_value(to_value(value)?).await
    }
    pub async fn submit_value(&self, value: Value) -> Result<WriteReceipt, ClientError> {
        write(
            &self.session,
            vec![WireOperation::SubmitCommand {
                topic: self.path.clone(),
                value: JsonValue::new(value),
            }],
        )
        .await
    }
}
/// Stages local operations. Dropping a builder sends nothing.
pub struct Batch {
    session: Weak<Inner>,
    operations: Vec<WireOperation>,
}
impl Batch {
    fn check<T, K: TopicKind>(&self, topic: &Topic<T, K>) -> Result<(), ClientError> {
        if self.session.ptr_eq(&topic.session) {
            Ok(())
        } else {
            Err(ClientError::WrongSession)
        }
    }
    pub fn publish<T: Serialize>(
        &mut self,
        topic: &StateTopic<T>,
        value: &T,
        expiry: ExpiryUpdate,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        let value = JsonValue::new(to_value(value)?);
        self.operations.push(WireOperation::PublishState {
            topic: topic.path.clone(),
            value,
            expiry: Some(expiry.into()),
        });
        Ok(self)
    }
    pub fn emit<T: Serialize>(
        &mut self,
        topic: &EventTopic<T>,
        value: &T,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        self.operations.push(WireOperation::PublishEvent {
            topic: topic.path.clone(),
            value: JsonValue::new(to_value(value)?),
        });
        Ok(self)
    }
    pub fn submit<T: Serialize>(
        &mut self,
        topic: &DesiredTopic<T>,
        value: &T,
        expiry: ExpiryUpdate,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        self.operations.push(WireOperation::SubmitDesired {
            topic: topic.path.clone(),
            value: JsonValue::new(to_value(value)?),
            expiry: Some(expiry.into()),
        });
        Ok(self)
    }
    pub fn command<T: Serialize>(
        &mut self,
        topic: &CommandTopic<T>,
        value: &T,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        self.operations.push(WireOperation::SubmitCommand {
            topic: topic.path.clone(),
            value: JsonValue::new(to_value(value)?),
        });
        Ok(self)
    }
    pub fn define<T, K: InputTopicKind>(
        &mut self,
        topic: &Topic<T, K>,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        self.operations.push(WireOperation::DefineInput {
            topic: topic.path.clone(),
            kind: K::INPUT,
        });
        Ok(self)
    }
    pub fn claim<T, K: InputTopicKind>(
        &mut self,
        topic: &Topic<T, K>,
        release: ClaimRelease,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        self.operations.push(WireOperation::ClaimInput {
            topic: topic.path.clone(),
            release: release.into(),
        });
        Ok(self)
    }
    pub fn remove<T, K: TopicKind>(
        &mut self,
        topic: &Topic<T, K>,
    ) -> Result<&mut Self, ClientError> {
        self.check(topic)?;
        self.operations.push(WireOperation::RemoveNode {
            topic: topic.path.clone(),
        });
        Ok(self)
    }
    pub async fn commit(self) -> Result<WriteReceipt, ClientError> {
        write(&self.session, self.operations).await
    }
}
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("topic was not selected for this observation: {topic}")]
    NotObserved { topic: TopicPath },
    #[error("wrong kind at {topic}: expected {expected:?}, got {actual:?}")]
    WrongKind {
        topic: TopicPath,
        expected: NodeKind,
        actual: NodeKind,
    },
    #[error("payload decoding failed at {topic}: {source}")]
    Payload {
        topic: TopicPath,
        #[source]
        source: PayloadError,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Current<T> {
    pub value: T,
    pub last_write: ProvenanceView,
    pub expires_at: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum DecodedNode<T> {
    State(Current<T>),
    Event {
        last_publisher: ProvenanceView,
    },
    Desired {
        definition: InputDefinitionView,
        claim: Option<ClaimView>,
        current: Option<Current<T>>,
    },
    Command {
        definition: InputDefinitionView,
        claim: Option<ClaimView>,
    },
}
impl<T> DecodedNode<T> {
    pub fn current(&self) -> Option<&Current<T>> {
        match self {
            Self::State(c) => Some(c),
            Self::Desired { current, .. } => current.as_ref(),
            _ => None,
        }
    }
}
pub(crate) fn node_kind(node: &NodeView) -> NodeKind {
    match node {
        NodeView::State { .. } => NodeKind::State,
        NodeView::Event { .. } => NodeKind::Event,
        NodeView::Desired { .. } => NodeKind::Desired,
        NodeView::Command { .. } => NodeKind::Command,
    }
}
fn decode_current<T: DeserializeOwned>(
    current: &CurrentValueView,
) -> Result<Current<T>, PayloadError> {
    Ok(Current {
        value: from_value(current.value.as_inner())?,
        last_write: current.last_write.clone(),
        expires_at: current.expires_at.clone(),
    })
}
fn decode_node<T: DeserializeOwned>(node: &NodeView) -> Result<DecodedNode<T>, PayloadError> {
    Ok(match node {
        NodeView::State { current } => DecodedNode::State(decode_current(current)?),
        NodeView::Event { last_publisher } => DecodedNode::Event {
            last_publisher: last_publisher.clone(),
        },
        NodeView::Desired {
            definition,
            claim,
            current,
        } => DecodedNode::Desired {
            definition: definition.clone(),
            claim: claim.clone(),
            current: current.as_ref().map(decode_current).transpose()?,
        },
        NodeView::Command { definition, claim } => DecodedNode::Command {
            definition: definition.clone(),
            claim: claim.clone(),
        },
    })
}
