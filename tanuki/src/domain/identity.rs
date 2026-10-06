pub use tanuki_protocol::{ClientName, Timestamp};
pub use tanuki_protocol::{ClientNameParseError, Deadline, NonNegativeDuration};
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(u64);

impl SessionId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClaimId(u64);

impl ClaimId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionHandle {
    id: SessionId,
    client: ClientName,
}

impl SessionHandle {
    pub(crate) fn new(id: SessionId, client: ClientName) -> Self {
        Self { id, client }
    }

    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.id
    }

    pub fn client(&self) -> &ClientName {
        &self.client
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteContext {
    Stateless { client: ClientName },
    Managed(SessionHandle),
}

impl WriteContext {
    #[must_use]
    pub fn stateless(client: ClientName) -> Self {
        Self::Stateless { client }
    }

    #[must_use]
    pub fn managed(handle: SessionHandle) -> Self {
        Self::Managed(handle)
    }

    pub fn client(&self) -> &ClientName {
        match self {
            Self::Stateless { client } => client,
            Self::Managed(handle) => handle.client(),
        }
    }

    pub(crate) fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::Stateless { .. } => None,
            Self::Managed(handle) => Some(handle.id()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteProvenance {
    client: ClientName,
    session: Option<SessionId>,
    at: Timestamp,
}

impl WriteProvenance {
    #[must_use]
    pub fn from_context(context: &WriteContext, at: Timestamp) -> Self {
        Self {
            client: context.client().clone(),
            session: context.session_id(),
            at,
        }
    }

    pub fn client(&self) -> &ClientName {
        &self.client
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        self.at
    }

    #[must_use]
    pub const fn session(&self) -> Option<SessionId> {
        self.session
    }
}
