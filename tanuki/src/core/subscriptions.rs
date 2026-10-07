//! Snapshot registration and bounded complete-commit delivery.
use super::*;

impl Core {
    pub fn subscribe(
        &mut self,
        selection: Selection,
        capacity: SubscriptionCapacity,
    ) -> Result<Subscription, CoreError> {
        self.subscribers
            .retain(|_, subscriber| !subscriber.updates.is_closed());
        let raw_id = self
            .next_subscription_id
            .checked_add(1)
            .ok_or(CoreError::SubscriptionIdExhausted)?;
        let id = SubscriptionId(raw_id);
        let snapshot = self.read(&selection);
        let (updates, receiver) = mpsc::channel(capacity.get());
        let end = Arc::new(Mutex::new(None));
        self.subscribers.insert(
            id,
            SubscriberState {
                selection,
                updates,
                end: Arc::clone(&end),
            },
        );
        self.next_subscription_id = raw_id;
        Ok(Subscription {
            id,
            snapshot,
            updates: receiver,
            end,
        })
    }

    pub(super) fn publish_update(&mut self, update: &UpdateBatch) {
        let mut ended = Vec::new();
        for (id, subscriber) in &self.subscribers {
            if subscriber.updates.is_closed() {
                ended.push(*id);
                continue;
            }
            let changes = update
                .changes
                .iter()
                .filter(|change| subscriber.selection.matches(change.topic()))
                .cloned()
                .collect::<Vec<_>>();
            if changes.is_empty() {
                continue;
            }
            let projected = UpdateBatch {
                sequence: update.sequence,
                changes,
            };
            match subscriber.updates.try_send(projected) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn!(subscription = id.get(), "slow subscription disconnected");
                    *subscriber
                        .end
                        .lock()
                        .expect("subscription end lock is not poisoned") =
                        Some(SubscriptionEnd::SlowConsumer);
                    ended.push(*id);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => ended.push(*id),
            }
        }
        for id in ended {
            self.subscribers.remove(&id);
        }
    }
}
