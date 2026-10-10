//! One disk/credential writer for the process. At most one waiting snapshot per
//! document is retained; a newer snapshot supersedes only waiting work. Running
//! writes finish first, so an older write can never overwrite a newer one.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Config,
    Credential,
    Portfolio,
    Journal,
    Treasure,
    Radar,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveState {
    Pending(u64),
    Saved(u64),
    Failed(u64, String),
}

impl SaveState {
    fn revision(&self) -> u64 {
        match self {
            Self::Pending(revision) | Self::Saved(revision) | Self::Failed(revision, _) => {
                *revision
            }
        }
    }
}

type Operation = Box<dyn FnOnce() -> Result<()> + Send + 'static>;
struct Job {
    revision: u64,
    operation: Operation,
}

#[derive(Default)]
struct State {
    next: u64,
    pending: BTreeMap<Slot, Job>,
    status: BTreeMap<Slot, SaveState>,
    active: bool,
    stopping: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct PersistenceWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PersistenceWorker {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("zstock-persistence".into())
            .spawn(move || run(worker))
            .expect("start persistence worker");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Nonblocking with respect to disk and credential operations. The mutex is
    /// held only to exchange a snapshot, never while an operation is executing.
    pub fn submit(
        &self,
        slot: Slot,
        operation: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> u64 {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.next += 1;
        let revision = state.next;
        state.pending.insert(
            slot,
            Job {
                revision,
                operation: Box::new(operation),
            },
        );
        state.status.insert(slot, SaveState::Pending(revision));
        self.shared.wake.notify_all();
        revision
    }

    pub fn status(&self) -> BTreeMap<Slot, SaveState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
            .clone()
    }

    /// A successful explicit recovery supersedes a prior failed ordinary write.
    /// Never erase a queued or running revision.
    pub fn clear_recovered_error(&self, slot: Slot) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(state.status.get(&slot), Some(SaveState::Failed(_, _))) {
            state.status.remove(&slot);
        }
    }

    /// Used off the UI thread by the quit hook. It drains all accepted work and
    /// reports failed latest revisions instead of interpreting an empty queue as
    /// successful persistence.
    pub fn flush(&self) -> Result<()> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.active || !state.pending.is_empty() {
            state = self
                .shared
                .wake
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let errors = state
            .status
            .iter()
            .filter_map(|(slot, status)| match status {
                SaveState::Failed(_, message)
                    if matches!(
                        slot,
                        Slot::Config | Slot::Credential | Slot::Portfolio | Slot::Journal
                    ) =>
                {
                    Some(format!("{slot:?}: {message}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
        Ok(())
    }
}

impl Default for PersistenceWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PersistenceWorker {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.stopping = true;
            self.shared.wake.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: Arc<Shared>) {
    loop {
        let (slot, job) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while state.pending.is_empty() && !state.stopping {
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            let Some(slot) = state
                .pending
                .iter()
                .min_by_key(|(_, job)| job.revision)
                .map(|(slot, _)| *slot)
            else {
                return;
            };
            state.active = true;
            (slot, state.pending.remove(&slot).expect("queued job"))
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.operation))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("persistence operation panicked")));
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .status
            .get(&slot)
            .is_some_and(|status| status.revision() == job.revision)
        {
            state.status.insert(
                slot,
                match result {
                    Ok(()) => SaveState::Saved(job.revision),
                    Err(error) => SaveState::Failed(job.revision, format!("{error:#}")),
                },
            );
        }
        state.active = false;
        shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn coalesces_waiting_snapshots_and_preserves_write_order() {
        let worker = PersistenceWorker::new();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let log = writes.clone();
        worker.submit(Slot::Portfolio, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            log.lock().unwrap().push(1);
            Ok(())
        });
        started_rx.recv().unwrap();
        for n in 2..=1_000 {
            let log = writes.clone();
            worker.submit(Slot::Portfolio, move || {
                log.lock().unwrap().push(n);
                Ok(())
            });
        }
        assert_eq!(worker.shared.state.lock().unwrap().pending.len(), 1);
        assert!(matches!(
            worker.status()[&Slot::Portfolio],
            SaveState::Pending(_)
        ));
        release_tx.send(()).unwrap();
        worker.flush().unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![1, 1_000]);
        assert!(matches!(
            worker.status()[&Slot::Portfolio],
            SaveState::Saved(_)
        ));
    }

    #[test]
    fn failed_write_is_reported_and_a_retry_can_succeed() {
        let worker = PersistenceWorker::new();
        worker.submit(Slot::Journal, || bail!("fixture full disk"));
        assert!(
            worker
                .flush()
                .unwrap_err()
                .to_string()
                .contains("fixture full disk")
        );
        assert!(matches!(
            worker.status()[&Slot::Journal],
            SaveState::Failed(_, _)
        ));
        worker.submit(Slot::Journal, || Ok(()));
        worker.flush().unwrap();
        assert!(matches!(
            worker.status()[&Slot::Journal],
            SaveState::Saved(_)
        ));
    }

    #[test]
    fn drop_drains_accepted_writes() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let log = writes.clone();
        {
            let worker = PersistenceWorker::new();
            worker.submit(Slot::Journal, move || {
                log.lock().unwrap().push("flushed");
                Ok(())
            });
        }
        assert_eq!(*writes.lock().unwrap(), vec!["flushed"]);
    }

    #[test]
    fn panic_does_not_leave_flush_waiting_forever() {
        let worker = PersistenceWorker::new();
        worker.submit(Slot::Config, || panic!("fixture panic"));
        assert!(worker.flush().is_err());
        worker.submit(Slot::Config, || Ok(()));
        worker.flush().unwrap();
    }
}
