//! Tracking of long-running operations started from the UI.
//!
//! An [`Operations`] set owns the abort handle of every operation it started,
//! keyed by what the operation is about (a vehicle and an operation kind, a
//! parameter, ...). That gives each feature, for free:
//! - "is this running?" per key, to disable buttons and show spinners;
//! - cancellation: [`Operations::cancel`] aborts the task, which drops the
//!   transaction's future and lets the middleware clean up (release locks,
//!   cancel commands and mission transfers on the vehicle);
//! - progress, for operations that report it;
//! - stale-result filtering: a result whose operation was cancelled or
//!   superseded is ignored by [`Operations::finish`].

use std::{collections::BTreeMap, future::Future, sync::Arc};

use futures::{StreamExt, channel::mpsc, stream};
use iced::Task;

use groundhog_logic::Progress;

/// A sink for progress reports, handed to operations that make progress.
pub type ProgressReporter = Arc<dyn Fn(Progress) + Send + Sync>;

struct Running {
    /// Aborts the task when dropped.
    _handle: iced::task::Handle,
    /// Distinguishes this run from a later one under the same key.
    generation: u64,
    progress: Option<Progress>,
}

/// The operations a feature has in flight, by key.
pub struct Operations<K> {
    running: BTreeMap<K, Running>,
    next_generation: u64,
}

impl<K> Default for Operations<K> {
    fn default() -> Self {
        Self {
            running: BTreeMap::new(),
            next_generation: 0,
        }
    }
}

/// A ticket identifying one run of an operation, carried by its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

impl<K: Ord + Clone + Send + 'static> Operations<K> {
    /// Start `future` under `key`, replacing (and aborting) any operation
    /// already running under it. `done` maps the output to a message carrying
    /// the [`Ticket`], to be passed to [`Operations::finish`].
    pub fn start<T, M>(
        &mut self,
        key: K,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(Ticket, T) -> M + Send + 'static,
    ) -> Task<M>
    where
        T: Send + 'static,
        M: Send + 'static,
    {
        let ticket = self.ticket();
        let (task, handle) = Task::perform(future, move |output| done(ticket, output)).abortable();
        self.insert(key, handle, ticket);
        task
    }

    /// Start an operation that reports progress. `operation` receives the
    /// [`ProgressReporter`] sink; `progress` maps each report to a message.
    pub fn start_with_progress<T, M, Fut>(
        &mut self,
        key: K,
        operation: impl FnOnce(ProgressReporter) -> Fut,
        progress: impl Fn(Ticket, Progress) -> M + Send + 'static,
        done: impl FnOnce(Ticket, T) -> M + Send + 'static,
    ) -> Task<M>
    where
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
        M: Send + 'static,
    {
        let ticket = self.ticket();
        let (sender, receiver) = mpsc::unbounded();
        let sink: ProgressReporter = Arc::new(move |report| {
            let _ = sender.unbounded_send(report);
        });

        // The sink is dropped with the operation's future, which ends the
        // progress stream, so the merged stream ends with the operation.
        let future = operation(sink);
        let reports = receiver.map(move |report| progress(ticket, report));
        let result = stream::once(future).map(move |output| (ticket, output));

        let mut done = Some(done);
        let merged = stream::select(
            reports.map(Some),
            result.map(move |(ticket, output)| done.take().map(|done| done(ticket, output))),
        )
        .filter_map(futures::future::ready);

        let (task, handle) = Task::stream(merged).abortable();
        self.insert(key, handle, ticket);
        task
    }

    fn ticket(&mut self) -> Ticket {
        self.next_generation += 1;
        Ticket(self.next_generation)
    }

    fn insert(&mut self, key: K, handle: iced::task::Handle, ticket: Ticket) {
        self.running.insert(
            key,
            Running {
                _handle: handle.abort_on_drop(),
                generation: ticket.0,
                progress: None,
            },
        );
    }

    /// Whether `ticket` is the current run of `key`.
    pub fn is_current(&self, key: &K, ticket: Ticket) -> bool {
        self.running
            .get(key)
            .is_some_and(|running| running.generation == ticket.0)
    }

    /// Record that the run `ticket` of `key` finished. Returns `false` for a
    /// stale result (cancelled or superseded), which the caller should drop.
    pub fn finish(&mut self, key: &K, ticket: Ticket) -> bool {
        if self.is_current(key, ticket) {
            self.running.remove(key);
            true
        } else {
            false
        }
    }

    /// Record a progress report of the run `ticket`. Stale reports are ignored.
    pub fn report(&mut self, key: &K, ticket: Ticket, progress: Progress) {
        if let Some(running) = self.running.get_mut(key)
            && running.generation == ticket.0
        {
            running.progress = Some(progress);
        }
    }

    /// Abort the operation under `key`. Returns whether one was running.
    pub fn cancel(&mut self, key: &K) -> bool {
        self.running.remove(key).is_some()
    }

    pub fn is_running(&self, key: &K) -> bool {
        self.running.contains_key(key)
    }

    /// The last progress report of the operation under `key`.
    pub fn progress(&self, key: &K) -> Option<Progress> {
        self.running.get(key).and_then(|running| running.progress)
    }

    /// The ticket of the run under `key`, for tests that simulate results.
    #[cfg(test)]
    pub fn ticket_of(&self, key: &K) -> Option<Ticket> {
        self.running
            .get(key)
            .map(|running| Ticket(running.generation))
    }

    /// Whether any key matching `matches` is running.
    pub fn any(&self, matches: impl Fn(&K) -> bool) -> bool {
        self.running.keys().any(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_results_are_rejected() {
        let mut operations = Operations::<u8>::default();

        let _first = operations.start(1, async {}, |ticket, ()| ticket);
        let first = Ticket(operations.next_generation);
        let _second = operations.start(1, async {}, |ticket, ()| ticket);
        let second = Ticket(operations.next_generation);

        assert!(!operations.finish(&1, first), "superseded");
        assert!(operations.is_running(&1));
        assert!(operations.finish(&1, second));
        assert!(!operations.is_running(&1));
    }

    #[test]
    fn cancelling_forgets_the_run() {
        let mut operations = Operations::<u8>::default();
        let _task = operations.start(3, async {}, |ticket, ()| ticket);
        let ticket = Ticket(operations.next_generation);

        assert!(operations.cancel(&3));
        assert!(
            !operations.finish(&3, ticket),
            "a cancelled run's result is stale"
        );
        assert!(!operations.cancel(&3));
    }

    #[test]
    fn progress_is_kept_per_run() {
        let mut operations = Operations::<u8>::default();
        let _task = operations.start(1, async {}, |ticket, ()| ticket);
        let ticket = Ticket(operations.next_generation);

        let report = Progress { done: 2, total: 5 };
        operations.report(&1, Ticket(999), report);
        assert_eq!(operations.progress(&1), None, "stale report ignored");
        operations.report(&1, ticket, report);
        assert_eq!(operations.progress(&1), Some(report));
    }
}
