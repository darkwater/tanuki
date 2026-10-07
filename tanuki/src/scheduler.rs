use std::sync::{Arc, Mutex};

use tokio::{sync::mpsc, time::sleep};
use tracing::warn;

use crate::{
    core::{Core, PendingClaimRelease},
    domain::{Deadline, Timestamp},
};

pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

#[derive(Clone, Debug)]
pub struct DeadlineScheduler {
    commands: mpsc::UnboundedSender<SchedulerCommand>,
    task: Arc<SchedulerTask>,
}

#[derive(Debug)]
struct SchedulerTask {
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    abort: tokio::task::AbortHandle,
}

impl Drop for SchedulerTask {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[derive(Debug)]
enum SchedulerCommand {
    Rescan,
    ScheduleClaims(Vec<PendingClaimRelease>),
    Stop,
}

impl DeadlineScheduler {
    #[must_use]
    pub fn start(core: Arc<Mutex<Core>>, clock: Clock) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(core, clock, receiver));
        let abort = task.abort_handle();
        Self {
            commands,
            task: Arc::new(SchedulerTask {
                join: Mutex::new(Some(task)),
                abort,
            }),
        }
    }

    pub(crate) fn abort(&self) {
        self.task.abort.abort();
    }

    pub async fn shutdown(&self) -> Result<(), tokio::task::JoinError> {
        let _ = self.commands.send(SchedulerCommand::Stop);
        let task = self
            .task
            .join
            .lock()
            .expect("scheduler task lock is not poisoned")
            .take();
        if let Some(task) = task {
            task.await?;
        }
        Ok(())
    }

    pub fn rescan(&self) {
        let _ = self.commands.send(SchedulerCommand::Rescan);
    }

    pub fn schedule_claims(&self, releases: &[PendingClaimRelease]) {
        if !releases.is_empty() {
            let _ = self
                .commands
                .send(SchedulerCommand::ScheduleClaims(releases.to_vec()));
        }
    }
}

async fn run(
    core: Arc<Mutex<Core>>,
    clock: Clock,
    mut commands: mpsc::UnboundedReceiver<SchedulerCommand>,
) {
    let mut claims = Vec::new();
    loop {
        let now = clock();
        let mut due_claims = Vec::new();
        claims.retain(|release: &PendingClaimRelease| {
            if release.deadline().get() <= now {
                due_claims.push(release.clone());
                false
            } else {
                true
            }
        });

        let next_value = match core.lock() {
            Ok(mut core) => {
                let should_process = !due_claims.is_empty()
                    || core
                        .next_value_deadline()
                        .is_some_and(|deadline| deadline.get() <= now);
                if should_process && let Err(error) = core.process_deadlines(now, &due_claims) {
                    warn!(%error, "deadline processing failed");
                }
                core.next_value_deadline()
            }
            Err(_) => {
                warn!("deadline scheduler stopped because the core lock was poisoned");
                return;
            }
        };
        let next_claim = claims.iter().map(PendingClaimRelease::deadline).min();
        let next = [next_value, next_claim].into_iter().flatten().min();

        let command = match next {
            None => commands.recv().await,
            Some(deadline) => {
                let wait = wait_duration(deadline, (clock)());
                tokio::select! {
                    command = commands.recv() => command,
                    () = sleep(wait) => Some(SchedulerCommand::Rescan),
                }
            }
        };
        match command {
            Some(SchedulerCommand::Rescan) => {}
            Some(SchedulerCommand::ScheduleClaims(releases)) => claims.extend(releases),
            Some(SchedulerCommand::Stop) | None => return,
        }
    }
}

fn wait_duration(deadline: Deadline, now: Timestamp) -> std::time::Duration {
    if deadline.get() <= now {
        std::time::Duration::ZERO
    } else {
        deadline
            .get()
            .get()
            .duration_since(now.get())
            .unsigned_abs()
    }
}
