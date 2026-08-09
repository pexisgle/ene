//! The scheduler timer task.
//!
//! The task is a timer only: it reads due schedules, sends
//! [`crate::handle::EneCommand::ScheduleFire`] into the actor mailbox, and
//! sleeps until the next due time. All database writes happen in the actor's
//! fire handler, so the single-flight gate and the claim transaction stay in
//! one place. The actor notifies the task through a watch channel after every
//! schedule mutation or fire processing; the task re-derives everything on
//! wakeup.

use crate::handle::EneCommand;
use crate::scheduler::SchedulerClock;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Upper bound on a single sleep, so a system clock adjustment or a missed
/// notification cannot stall dispatch indefinitely.
const MAX_SLEEP: Duration = Duration::from_hours(1);

pub(crate) async fn run(
    store: Arc<ene_store::MemoryStore>,
    cmd_tx: mpsc::UnboundedSender<EneCommand>,
    mut notify_rx: watch::Receiver<()>,
    clock: SchedulerClock,
) {
    // Consume the initial channel value so the first `changed()` below waits
    // for an actual notification instead of resolving immediately.
    notify_rx.borrow_and_update();
    let mut queued: HashSet<i64> = HashSet::new();
    loop {
        let now = clock();
        match store.list_due_schedules(now).await {
            Ok(due) => {
                for schedule in due {
                    if queued.contains(&schedule.id) {
                        continue;
                    }
                    let Some(scheduled_at) = schedule.next_run_at else {
                        continue;
                    };
                    if cmd_tx
                        .send(EneCommand::ScheduleFire {
                            schedule_id: schedule.id,
                            scheduled_at,
                        })
                        .is_err()
                    {
                        return; // actor is gone; nothing left to dispatch
                    }
                    queued.insert(schedule.id);
                }
            }
            Err(e) => {
                tracing::warn!(component = "Scheduler", error = %e, "Failed to list due schedules");
            }
        }

        let excluded: Vec<i64> = queued.iter().copied().collect();
        let next_due = match store.next_due_time_excluding(&excluded).await {
            Ok(next) => next,
            Err(e) => {
                tracing::warn!(component = "Scheduler", error = %e, "Failed to read next due time");
                None
            }
        };
        let sleep = next_due.map_or(MAX_SLEEP, |t| {
            // Clamp before converting so a far-future occurrence (e.g. a
            // cron year field) cannot overflow `Duration` and spin the loop.
            let secs = t
                .signed_duration_since(now)
                .num_seconds()
                .clamp(0, MAX_SLEEP.as_secs() as i64);
            Duration::from_secs(secs as u64)
        });

        tokio::select! {
            changed = notify_rx.changed() => {
                if changed.is_err() {
                    return; // actor is gone (sender dropped)
                }
                // DB state changed (a fire was processed or a schedule was
                // mutated): re-derive everything, including which fires are
                // still worth dispatching.
                queued.clear();
            }
            () = tokio::time::sleep(sleep) => {
                // Due check happens on the next loop iteration.
            }
        }
    }
}
