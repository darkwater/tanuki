//! Latest-state observation: complete, coherent snapshots of selected topics.
use crate::{
    Node, ReadError, Topic, TopicKind,
    protocol::{FromValue, NodeView, PayloadError, Selector, SnapshotView, TopicPath},
    session::Shared,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use tokio::sync::watch;

/// The retained nodes matching an observer's selectors after a complete
/// server commit. Cheap to clone; never changes once received.
#[derive(Clone, Debug)]
pub struct Snapshot {
    sequence: u64,
    nodes: Arc<BTreeMap<TopicPath, NodeView>>,
    selectors: Arc<[Selector]>,
}

impl Snapshot {
    pub(crate) fn project(
        sequence: u64,
        cache: &BTreeMap<TopicPath, NodeView>,
        selectors: &Arc<[Selector]>,
    ) -> Self {
        Self {
            sequence,
            nodes: Arc::new(
                cache
                    .iter()
                    .filter(|(path, _)| selectors.iter().any(|s| s.matches(path)))
                    .map(|(path, node)| (path.clone(), node.clone()))
                    .collect(),
            ),
            selectors: Arc::clone(selectors),
        }
    }

    /// The server commit this snapshot reflects. Sequences increase but skip
    /// commits that touched nothing selected.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every observed node, in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&TopicPath, &NodeView)> {
        self.nodes.iter()
    }

    #[must_use]
    pub fn node(&self, path: &TopicPath) -> Option<&NodeView> {
        self.nodes.get(path)
    }

    /// Whether this observer's selectors cover `path`. A covered path with no
    /// node simply does not exist right now.
    #[must_use]
    pub fn covers(&self, path: &TopicPath) -> bool {
        self.selectors.iter().any(|selector| selector.matches(path))
    }

    /// The current value of a state topic, or the pending request of a desired
    /// topic. `Ok(None)` means there is no such value right now.
    pub fn get<T: FromValue, K: TopicKind>(
        &self,
        topic: &Topic<T, K>,
    ) -> Result<Option<T>, ReadError> {
        Ok(self.read(topic)?.and_then(Node::into_value))
    }

    /// The complete decoded node, including provenance, expiry and claim.
    pub fn read<T: FromValue, K: TopicKind>(
        &self,
        topic: &Topic<T, K>,
    ) -> Result<Option<Node<T>>, ReadError> {
        if !self.covers(topic.path()) {
            return Err(ReadError::NotObserved {
                topic: topic.path().clone(),
            });
        }
        self.node(topic.path())
            .map(|node| topic.decode(node))
            .transpose()
    }

    /// Every observed node that has a current value (state values and pending
    /// desired requests), decoded into `T`. Handy for wildcard observations
    /// such as `/battery/*`.
    pub fn values<T: FromValue>(
        &self,
    ) -> impl Iterator<Item = (&TopicPath, Result<T, PayloadError>)> {
        self.nodes
            .iter()
            .filter_map(|(path, node)| Some((path, node.current()?.decode())))
    }
}

/// A snapshot covering every topic, such as a test fixture or a model fed
/// from another source.
impl From<SnapshotView> for Snapshot {
    fn from(view: SnapshotView) -> Self {
        Self {
            sequence: view.sequence,
            nodes: Arc::new(view.nodes),
            selectors: Arc::from([Selector::parse("/**").expect("a valid selector")]),
        }
    }
}

/// The plain wire shape, for code that maintains its own model.
impl From<&Snapshot> for SnapshotView {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            sequence: snapshot.sequence,
            nodes: (*snapshot.nodes).clone(),
        }
    }
}

/// Delivers the latest [`Snapshot`] of its selectors. A slow reader skips
/// intermediate snapshots but never sees half of a commit. Dropping the
/// observer unregisters it.
pub struct Observer {
    pub(crate) id: u64,
    pub(crate) snapshots: watch::Receiver<Option<Snapshot>>,
    pub(crate) shared: Weak<Shared>,
    pub(crate) started: bool,
}

impl Observer {
    /// Waits for the next snapshot. The first call returns as soon as the
    /// server has confirmed data for these selectors; later calls wait for a
    /// commit that changes them. While disconnected nothing new arrives;
    /// observers resume with a fresh snapshot after a reconnect. Returns
    /// `None` once the session is closed.
    pub async fn recv(&mut self) -> Option<Snapshot> {
        if !self.started {
            let first = self.snapshots.wait_for(Option::is_some).await.ok()?.clone();
            self.started = true;
            return first;
        }
        self.snapshots.changed().await.ok()?;
        self.snapshots.borrow_and_update().clone()
    }

    /// The most recent snapshot without waiting, if one has arrived.
    #[must_use]
    pub fn latest(&self) -> Option<Snapshot> {
        self.snapshots.borrow().clone()
    }

    /// Adapts the observer to a standard `Stream`; pin it before polling.
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Snapshot> {
        futures_util::stream::unfold(self, |mut observer| async move {
            observer.recv().await.map(|snapshot| (snapshot, observer))
        })
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.unregister(self.id);
        }
    }
}
