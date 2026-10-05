use super::{ClaimId, ClaimRelease, ClientName, Deadline, SessionId, Value, WriteProvenance};

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    State(StateNode),
    Event(EventNode),
    Desired(DesiredNode),
    Command(CommandNode),
}

impl Node {
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        match self {
            Self::State(_) => NodeKind::State,
            Self::Event(_) => NodeKind::Event,
            Self::Desired(_) => NodeKind::Desired,
            Self::Command(_) => NodeKind::Command,
        }
    }

    #[must_use]
    pub fn retained_value(&self) -> Option<&RetainedValue> {
        match self {
            Self::State(state) => Some(&state.current),
            Self::Desired(desired) => desired.current.as_ref(),
            Self::Event(_) | Self::Command(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    State,
    Event,
    Desired,
    Command,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetainedValue {
    value: Value,
    last_write: WriteProvenance,
    expires_at: Option<Deadline>,
}

impl RetainedValue {
    #[must_use]
    pub fn new(value: Value, last_write: WriteProvenance, expires_at: Option<Deadline>) -> Self {
        Self {
            value,
            last_write,
            expires_at,
        }
    }

    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    #[must_use]
    pub fn last_write(&self) -> &WriteProvenance {
        &self.last_write
    }

    #[must_use]
    pub const fn expires_at(&self) -> Option<Deadline> {
        self.expires_at
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateNode {
    current: RetainedValue,
}

impl StateNode {
    #[must_use]
    pub fn new(current: RetainedValue) -> Self {
        Self { current }
    }

    #[must_use]
    pub fn current(&self) -> &RetainedValue {
        &self.current
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventNode {
    last_publisher: WriteProvenance,
}

impl EventNode {
    #[must_use]
    pub fn new(last_publisher: WriteProvenance) -> Self {
        Self { last_publisher }
    }

    #[must_use]
    pub fn last_publisher(&self) -> &WriteProvenance {
        &self.last_publisher
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventOccurrence {
    value: Value,
    publisher: WriteProvenance,
}

impl EventOccurrence {
    #[must_use]
    pub fn new(value: Value, publisher: WriteProvenance) -> Self {
        Self { value, publisher }
    }

    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    #[must_use]
    pub fn publisher(&self) -> &WriteProvenance {
        &self.publisher
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct InputDefinition {
    private: (),
}

impl InputDefinition {
    #[must_use]
    pub const fn new() -> Self {
        Self { private: () }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputClaim {
    id: ClaimId,
    owner: ClientName,
    session: SessionId,
    release: ClaimRelease,
}

impl InputClaim {
    #[must_use]
    pub const fn id(&self) -> ClaimId {
        self.id
    }

    #[must_use]
    pub fn owner(&self) -> &ClientName {
        &self.owner
    }

    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    #[must_use]
    pub const fn release(&self) -> ClaimRelease {
        self.release
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesiredNode {
    definition: InputDefinition,
    claim: Option<InputClaim>,
    current: Option<RetainedValue>,
}

impl DesiredNode {
    #[must_use]
    pub fn new(definition: InputDefinition) -> Self {
        Self {
            definition,
            claim: None,
            current: None,
        }
    }

    #[must_use]
    pub fn with_current(definition: InputDefinition, current: RetainedValue) -> Self {
        Self {
            definition,
            claim: None,
            current: Some(current),
        }
    }

    #[must_use]
    pub fn definition(&self) -> &InputDefinition {
        &self.definition
    }

    #[must_use]
    pub fn current(&self) -> Option<&RetainedValue> {
        self.current.as_ref()
    }

    #[must_use]
    pub fn claim(&self) -> Option<&InputClaim> {
        self.claim.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandNode {
    definition: InputDefinition,
    claim: Option<InputClaim>,
}

impl CommandNode {
    #[must_use]
    pub fn new(definition: InputDefinition) -> Self {
        Self {
            definition,
            claim: None,
        }
    }

    #[must_use]
    pub fn definition(&self) -> &InputDefinition {
        &self.definition
    }

    #[must_use]
    pub fn claim(&self) -> Option<&InputClaim> {
        self.claim.as_ref()
    }
}
