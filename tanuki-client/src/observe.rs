use crate::{
    ClientError, DecodedNode, ReadError, SelectedView, Session, SessionEnd, Topic, TopicKind,
    protocol::*,
};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use tokio::{sync::watch, task::AbortHandle};
/// Immutable retained state after a complete batch. Occurrences remain on raw listeners.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservationSnapshot {
    sequence: u64,
    nodes: BTreeMap<TopicPath, NodeView>,
    paths: BTreeSet<TopicPath>,
}
impl ObservationSnapshot {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn nodes(&self) -> &BTreeMap<TopicPath, NodeView> {
        &self.nodes
    }
    pub fn paths(&self) -> &BTreeSet<TopicPath> {
        &self.paths
    }
    pub fn node(&self, path: &TopicPath) -> Option<&NodeView> {
        self.nodes.get(path)
    }
    pub fn read<T: DeserializeOwned, K: TopicKind>(
        &self,
        topic: &Topic<T, K>,
    ) -> Result<Option<DecodedNode<T>>, ReadError> {
        if !self.paths.contains(topic.path()) {
            return Err(ReadError::NotObserved {
                topic: topic.path().clone(),
            });
        }
        self.node(topic.path())
            .map(|node| topic.decode(node))
            .transpose()
    }
}
pub struct Observer {
    initial: Option<ObservationSnapshot>,
    snapshots: watch::Receiver<ObservationSnapshot>,
    terminal: watch::Receiver<Option<SessionEnd>>,
    task: AbortHandle,
    ended: bool,
    seen_sequence: u64,
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Observer {
    pub async fn recv(&mut self) -> Result<Option<ObservationSnapshot>, SessionEnd> {
        if self.ended {
            return Ok(None);
        }
        if let Some(initial) = self.initial.take() {
            return Ok(Some(initial));
        }
        loop {
            if let Some(reason) = self.terminal.borrow().clone() {
                if matches!(reason, SessionEnd::Closed) {
                    let latest = self.snapshots.borrow_and_update().clone();
                    if latest.sequence() > self.seen_sequence {
                        self.seen_sequence = latest.sequence();
                        return Ok(Some(latest));
                    }
                }
                self.ended = true;
                return if matches!(reason, SessionEnd::Closed) {
                    Ok(None)
                } else {
                    Err(reason)
                };
            }
            tokio::select! { biased;
                result = self.terminal.changed() => { if result.is_err() { self.ended = true; return Ok(None); } }
                result = self.snapshots.changed() => {
                    if result.is_err() { self.ended = true; return Ok(None); }
                    let latest = self.snapshots.borrow_and_update().clone();
                    self.seen_sequence = latest.sequence();
                    return Ok(Some(latest));
                }
            }
        }
    }
    pub fn into_stream(
        self,
    ) -> impl futures_util::Stream<Item = Result<ObservationSnapshot, SessionEnd>> {
        futures_util::stream::unfold(self, |mut observer| async move {
            match observer.recv().await {
                Ok(Some(snapshot)) => Some((Ok(snapshot), observer)),
                Err(error) => Some((Err(error), observer)),
                Ok(None) => None,
            }
        })
    }
}
impl Session {
    pub async fn observe<T, K: TopicKind>(
        &self,
        topic: &Topic<T, K>,
    ) -> Result<Observer, ClientError> {
        topic.check_session(self)?;
        self.observe_batch([topic.path().clone()]).await
    }
    pub async fn observe_batch(
        &self,
        paths: impl IntoIterator<Item = TopicPath>,
    ) -> Result<Observer, ClientError> {
        let paths: BTreeSet<_> = paths.into_iter().collect();
        for path in &paths {
            if !self.selection().matches(path) {
                return Err(ClientError::NotSubscribed(path.clone()));
            }
        }
        let raw = self.listen_updates().await?;
        let (observer, task) = projection(raw, paths);
        let mut tasks = self.inner.projections.lock().unwrap();
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
        Ok(observer)
    }
}

fn project(view: &SelectedView, paths: &BTreeSet<TopicPath>) -> ObservationSnapshot {
    ObservationSnapshot {
        sequence: view.sequence(),
        paths: paths.clone(),
        nodes: paths
            .iter()
            .filter_map(|path| {
                view.nodes()
                    .get(&path.to_string())
                    .map(|node| (path.clone(), node.clone()))
            })
            .collect(),
    }
}
fn change_topic(change: &ChangeView) -> &str {
    match change {
        ChangeView::Upsert { topic, .. }
        | ChangeView::Removed { topic, .. }
        | ChangeView::Event { topic, .. }
        | ChangeView::Command { topic, .. } => topic,
    }
}

// Concrete projection seam for deterministic channel tests; the public primitive is RawListener.
fn projection(
    mut raw: crate::RawListener,
    paths: BTreeSet<TopicPath>,
) -> (Observer, tokio::task::JoinHandle<()>) {
    let baseline = raw.initial_snapshot();
    let nodes = paths
        .iter()
        .filter_map(|path| {
            baseline
                .nodes
                .get(&path.to_string())
                .map(|node| (path.to_string(), node.clone()))
        })
        .collect();
    let mut view = SelectedView::from_snapshot(SnapshotView {
        sequence: baseline.sequence,
        nodes,
    });
    let snapshot = project(&view, &paths);
    let (snapshots, rx) = watch::channel(snapshot.clone());
    let (terminal, end) = watch::channel(None);
    let task = tokio::spawn(async move {
        let names: BTreeSet<_> = paths.iter().map(ToString::to_string).collect();
        let reason = loop {
            let update = match raw.recv().await {
                Ok(Some(update)) => update,
                Ok(None) => break SessionEnd::Closed,
                Err(reason) => break reason,
            };
            let changes = update
                .changes
                .into_iter()
                .filter(|change| names.contains(change_topic(change)))
                .collect::<Vec<_>>();
            let matching = !changes.is_empty();
            if let Err(error) = view.apply(UpdateView {
                sequence: update.sequence,
                changes,
            }) {
                break SessionEnd::Sequence(error);
            }
            if matching {
                snapshots.send_replace(project(&view, &paths));
            }
        };
        terminal.send_replace(Some(reason));
    });
    let abort = task.abort_handle();
    (
        Observer {
            seen_sequence: snapshot.sequence(),
            initial: Some(snapshot),
            snapshots: rx,
            terminal: end,
            task: abort,
            ended: false,
        },
        task,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::raw_channel;
    fn upsert(topic: &str, value: i64) -> ChangeView {
        ChangeView::Upsert {
            topic: topic.into(),
            node: NodeView::State {
                current: CurrentValueView {
                    value: JsonValue::new(Value::Integer(value)),
                    last_write: ProvenanceView {
                        client: "fixture".into(),
                        session: None,
                        at: "2023-11-14T22:13:20Z".into(),
                    },
                    expires_at: None,
                },
            },
        }
    }
    #[tokio::test]
    async fn unread_outputs_coalesce_but_each_input_delta_is_applied() {
        let (raw, input, end) = raw_channel(
            SnapshotView {
                sequence: 0,
                nodes: Default::default(),
            },
            64,
        );
        let paths = BTreeSet::from([
            TopicPath::parse("/lamp/hue").unwrap(),
            TopicPath::parse("/lamp/brightness").unwrap(),
        ]);
        let (mut observer, task) = projection(raw, paths);
        // Both writes occur before the consumer first polls; the empty baseline must survive.
        input
            .send(UpdateView {
                sequence: 5,
                changes: vec![upsert("/lamp/hue", 120)],
            })
            .await
            .unwrap();
        input
            .send(UpdateView {
                sequence: 20,
                changes: vec![upsert("/lamp/brightness", 70)],
            })
            .await
            .unwrap();
        input
            .send(UpdateView {
                sequence: 50,
                changes: vec![upsert("/lamp/hue", 35)],
            })
            .await
            .unwrap();
        let mut barrier = observer.snapshots.clone();
        barrier.wait_for(|s| s.sequence() == 50).await.unwrap();
        assert!(observer.recv().await.unwrap().unwrap().nodes().is_empty());
        let latest = observer.recv().await.unwrap().unwrap();
        assert_eq!(latest.sequence(), 50);
        assert_eq!(latest.nodes().len(), 2);
        let NodeView::State { current } = latest
            .node(&TopicPath::parse("/lamp/brightness").unwrap())
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(current.value.as_inner(), &Value::Integer(70));
        end.send_replace(Some(SessionEnd::Closed));
        task.await.unwrap();
    }
    #[tokio::test]
    async fn clean_projection_closure_delivers_the_last_unseen_snapshot() {
        let (raw, input, end) = raw_channel(
            SnapshotView {
                sequence: 0,
                nodes: Default::default(),
            },
            2,
        );
        let (mut observer, task) = projection(
            raw,
            BTreeSet::from([TopicPath::parse("/lamp/hue").unwrap()]),
        );
        input
            .try_send(UpdateView {
                sequence: 5,
                changes: vec![upsert("/lamp/hue", 120)],
            })
            .unwrap();
        end.send_replace(Some(SessionEnd::Closed));
        drop(input);
        task.await.unwrap();
        assert!(observer.recv().await.unwrap().unwrap().nodes().is_empty());
        assert_eq!(observer.recv().await.unwrap().unwrap().sequence(), 5);
        assert!(observer.recv().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn projection_input_lag_is_terminal_even_with_unread_output() {
        let (raw, input, end) = raw_channel(
            SnapshotView {
                sequence: 0,
                nodes: Default::default(),
            },
            1,
        );
        let (mut observer, task) = projection(
            raw,
            BTreeSet::from([TopicPath::parse("/lamp/hue").unwrap()]),
        );
        // Fill before scheduling projection, precisely reproducing the driver's try_send failure.
        input
            .try_send(UpdateView {
                sequence: 1,
                changes: vec![upsert("/lamp/hue", 1)],
            })
            .unwrap();
        assert!(
            input
                .try_send(UpdateView {
                    sequence: 2,
                    changes: vec![upsert("/lamp/hue", 2)]
                })
                .is_err()
        );
        end.send_replace(Some(SessionEnd::Lagged));
        task.await.unwrap();
        assert!(observer.recv().await.unwrap().is_some());
        assert!(matches!(observer.recv().await, Err(SessionEnd::Lagged)));
        assert!(observer.recv().await.unwrap().is_none());
    }
}
