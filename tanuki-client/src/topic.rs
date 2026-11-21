//! Typed, session-independent topic handles and what reading one yields.
use crate::protocol::*;
use std::{fmt, hash::Hash, marker::PhantomData, time::Duration};

mod sealed {
    pub trait Kind {}
    pub trait Source {}
}

/// One of the four node kinds, as a type parameter of [`Topic`].
pub trait TopicKind: sealed::Kind + Send + Sync + 'static {
    const KIND: NodeKind;
}
/// Desired and command topics: inputs that are defined, claimed and submitted to.
pub trait InputKind: TopicKind {
    const INPUT: WireInputKind;
}
/// Event and command topics: occurrences delivered once, never retained.
pub trait InstantKind: TopicKind {}

/// Retained output: the latest value, owned by its last writer.
pub enum State {}
/// Instant output: each occurrence is delivered once.
pub enum Event {}
/// Retained input: the latest request for a controller to act on.
pub enum Desired {}
/// Instant input: each submission is delivered once to listeners.
pub enum Command {}

macro_rules! kinds {
    ($($kind:ident),*) => {$(
        impl sealed::Kind for $kind {}
        impl TopicKind for $kind {
            const KIND: NodeKind = NodeKind::$kind;
        }
    )*};
}
kinds!(State, Event, Desired, Command);
impl InputKind for Desired {
    const INPUT: WireInputKind = WireInputKind::Desired;
}
impl InputKind for Command {
    const INPUT: WireInputKind = WireInputKind::Command;
}
impl InstantKind for Event {}
impl InstantKind for Command {}

/// A typed name for one topic. Handles are plain values: create them anywhere
/// (before connecting, in statics via `LazyLock`, in configuration) and use
/// them with any [`Session`](crate::Session). Creating one sends nothing.
///
/// `T` is the payload type, converted with Serde through [`to_value`] and
/// [`from_value`]; use [`Value`] to handle arbitrary payloads.
pub struct Topic<T, K: TopicKind> {
    path: TopicPath,
    marker: PhantomData<fn() -> (T, K)>,
}
pub type StateTopic<T> = Topic<T, State>;
pub type EventTopic<T> = Topic<T, Event>;
pub type DesiredTopic<T> = Topic<T, Desired>;
pub type CommandTopic<T> = Topic<T, Command>;

impl<T, K: TopicKind> Topic<T, K> {
    /// Parses `path`, such as `"/lamp/brightness"`.
    pub fn new(path: &str) -> Result<Self, PathParseError> {
        TopicPath::parse(path).map(Self::from_path)
    }

    #[must_use]
    pub fn from_path(path: TopicPath) -> Self {
        Self {
            path,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn path(&self) -> &TopicPath {
        &self.path
    }

    /// The selector matching exactly this topic.
    #[must_use]
    pub fn selector(&self) -> Selector {
        Selector::from(&self.path)
    }

    /// Decodes a node of this topic, checking its kind.
    pub fn decode(&self, node: &NodeView) -> Result<Node<T>, ReadError>
    where
        T: FromValue,
    {
        if node.kind() != K::KIND {
            return Err(ReadError::WrongKind {
                topic: self.path.clone(),
                expected: K::KIND,
                actual: node.kind(),
            });
        }
        Node::decode(node).map_err(|source| ReadError::Payload {
            topic: self.path.clone(),
            source,
        })
    }
}

impl<T, K: TopicKind> Clone for Topic<T, K> {
    fn clone(&self) -> Self {
        Self::from_path(self.path.clone())
    }
}
impl<T, K: TopicKind> fmt::Debug for Topic<T, K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}Topic({})", K::KIND, self.path)
    }
}
impl<T, K: TopicKind> fmt::Display for Topic<T, K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.fmt(formatter)
    }
}
impl<T, K: TopicKind> PartialEq for Topic<T, K> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
impl<T, K: TopicKind> Eq for Topic<T, K> {}
impl<T, K: TopicKind> Hash for Topic<T, K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}
impl<T, K: TopicKind> TryFrom<&str> for Topic<T, K> {
    type Error = PathParseError;
    fn try_from(path: &str) -> Result<Self, Self::Error> {
        Self::new(path)
    }
}
impl<T, K: TopicKind> From<TopicPath> for Topic<T, K> {
    fn from(path: TopicPath) -> Self {
        Self::from_path(path)
    }
}

/// When a retained value (state or desired) stops being current.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Expiry {
    /// The value stays until replaced or removed.
    #[default]
    Never,
    /// The server removes the value this long after the write (a lease).
    /// Publishing again before then renews it.
    After(Duration),
    /// Keep the deadline of the value being replaced, if any.
    Keep,
}

/// What happens to an input claim when the claiming session disconnects.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClaimRelease {
    #[default]
    Immediate,
    /// Keep the claim this long, so a quick reconnect keeps control.
    After(Duration),
}

/// A `std` duration beyond the server's signed 64-bit-second range.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("duration {0:?} is too long")]
pub struct DurationOutOfRange(pub Duration);

pub(crate) fn iso_duration(duration: Duration) -> Result<String, DurationOutOfRange> {
    jiff::SignedDuration::try_from(duration)
        .map(|duration| duration.to_string())
        .map_err(|_| DurationOutOfRange(duration))
}

impl TryFrom<Expiry> for WireExpiry {
    type Error = DurationOutOfRange;
    fn try_from(value: Expiry) -> Result<Self, Self::Error> {
        Ok(match value {
            Expiry::Never => Self::Clear,
            Expiry::Keep => Self::Preserve,
            Expiry::After(duration) => Self::Set {
                duration: iso_duration(duration)?,
            },
        })
    }
}
impl TryFrom<ClaimRelease> for WireRelease {
    type Error = DurationOutOfRange;
    fn try_from(value: ClaimRelease) -> Result<Self, Self::Error> {
        Ok(match value {
            ClaimRelease::Immediate => Self::Immediate,
            ClaimRelease::After(duration) => Self::After {
                duration: iso_duration(duration)?,
            },
        })
    }
}

/// A retained value with its metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Current<T> {
    pub value: T,
    pub last_write: ProvenanceView,
    pub expires_at: Option<jiff::Timestamp>,
}

/// A node with its payload decoded into `T`.
#[derive(Clone, Debug, PartialEq)]
pub enum Node<T> {
    State(Current<T>),
    Event {
        last_publisher: ProvenanceView,
    },
    Desired {
        claim: Option<ClaimView>,
        current: Option<Current<T>>,
    },
    Command {
        claim: Option<ClaimView>,
    },
}

impl<T> Node<T> {
    /// The state value or desired request, if there is one.
    pub fn current(&self) -> Option<&Current<T>> {
        match self {
            Self::State(current) => Some(current),
            Self::Desired { current, .. } => current.as_ref(),
            Self::Event { .. } | Self::Command { .. } => None,
        }
    }

    pub fn into_value(self) -> Option<T> {
        match self {
            Self::State(current) => Some(current.value),
            Self::Desired { current, .. } => current.map(|current| current.value),
            Self::Event { .. } | Self::Command { .. } => None,
        }
    }
}

impl<T: FromValue> Node<T> {
    fn decode(node: &NodeView) -> Result<Self, PayloadError> {
        let current = |current: &CurrentValueView| -> Result<Current<T>, PayloadError> {
            Ok(Current {
                value: current.decode()?,
                last_write: current.last_write.clone(),
                expires_at: current.expires_at,
            })
        };
        Ok(match node {
            NodeView::State { current: value } => Self::State(current(value)?),
            NodeView::Event { last_publisher } => Self::Event {
                last_publisher: last_publisher.clone(),
            },
            NodeView::Desired {
                claim,
                current: value,
                ..
            } => Self::Desired {
                claim: claim.clone(),
                current: value.as_ref().map(current).transpose()?,
            },
            NodeView::Command { claim, .. } => Self::Command {
                claim: claim.clone(),
            },
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{topic} is not covered by this observer's selectors")]
    NotObserved { topic: TopicPath },
    #[error("{topic} is a {actual:?} node, not {expected:?}")]
    WrongKind {
        topic: TopicPath,
        expected: NodeKind,
        actual: NodeKind,
    },
    #[error("could not decode {topic}: {source}")]
    Payload {
        topic: TopicPath,
        #[source]
        source: PayloadError,
    },
}

/// Anything naming a set of topics: selector text such as `"/battery/*"`, a
/// [`Selector`], a [`TopicPath`], or a [`Topic`] handle (exactly that topic).
pub trait SelectorSource: sealed::Source {
    fn to_selector(&self) -> Result<Selector, SelectorParseError>;
}
impl sealed::Source for str {}
impl SelectorSource for str {
    fn to_selector(&self) -> Result<Selector, SelectorParseError> {
        Selector::parse(self)
    }
}
impl sealed::Source for String {}
impl SelectorSource for String {
    fn to_selector(&self) -> Result<Selector, SelectorParseError> {
        Selector::parse(self)
    }
}
impl sealed::Source for Selector {}
impl SelectorSource for Selector {
    fn to_selector(&self) -> Result<Selector, SelectorParseError> {
        Ok(self.clone())
    }
}
impl sealed::Source for TopicPath {}
impl SelectorSource for TopicPath {
    fn to_selector(&self) -> Result<Selector, SelectorParseError> {
        Ok(Selector::from(self))
    }
}
impl<T, K: TopicKind> sealed::Source for Topic<T, K> {}
impl<T, K: TopicKind> SelectorSource for Topic<T, K> {
    fn to_selector(&self) -> Result<Selector, SelectorParseError> {
        Ok(self.selector())
    }
}
impl<S: SelectorSource + ?Sized> sealed::Source for &S {}
impl<S: SelectorSource + ?Sized> SelectorSource for &S {
    fn to_selector(&self) -> Result<Selector, SelectorParseError> {
        (**self).to_selector()
    }
}

/// One selector source or a collection of them, as accepted by
/// [`Session::observe`](crate::Session::observe) and friends:
/// `"/battery/*"`, `&topic`, `["/a/*", "/b"]`, a tuple of different kinds
/// such as `(&hue, &brightness, "/battery/*")`, or a `Vec`/slice.
pub trait IntoSelectors {
    fn into_selectors(self) -> Result<Vec<Selector>, SelectorParseError>;
}
impl<S: SelectorSource> IntoSelectors for S {
    fn into_selectors(self) -> Result<Vec<Selector>, SelectorParseError> {
        Ok(vec![self.to_selector()?])
    }
}
impl<S: SelectorSource, const N: usize> IntoSelectors for [S; N] {
    fn into_selectors(self) -> Result<Vec<Selector>, SelectorParseError> {
        self.iter().map(SelectorSource::to_selector).collect()
    }
}
impl<S: SelectorSource> IntoSelectors for Vec<S> {
    fn into_selectors(self) -> Result<Vec<Selector>, SelectorParseError> {
        self.iter().map(SelectorSource::to_selector).collect()
    }
}
impl<S: SelectorSource> IntoSelectors for &[S] {
    fn into_selectors(self) -> Result<Vec<Selector>, SelectorParseError> {
        self.iter().map(SelectorSource::to_selector).collect()
    }
}

macro_rules! tuple_selectors {
    ($($name:ident),+) => {
        impl<$($name: SelectorSource),+> IntoSelectors for ($($name,)+) {
            #[allow(non_snake_case, reason = "the type parameters name the tuple fields")]
            fn into_selectors(self) -> Result<Vec<Selector>, SelectorParseError> {
                let ($($name,)+) = self;
                Ok(vec![$($name.to_selector()?),+])
            }
        }
    };
}
tuple_selectors!(A, B);
tuple_selectors!(A, B, C);
tuple_selectors!(A, B, C, D);
tuple_selectors!(A, B, C, D, E);
tuple_selectors!(A, B, C, D, E, F);
tuple_selectors!(A, B, C, D, E, F, G);
tuple_selectors!(A, B, C, D, E, F, G, H);
