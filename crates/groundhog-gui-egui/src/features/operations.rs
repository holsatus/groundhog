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
//!
//! Where the iced frontend maps a finished operation onto the message
//! system, here a spawned task sends the outcome into the feature's own
//! channel, and the feature drains it each frame. The ticket filtering is
//! identical.

use std::{collections::BTreeMap, future::Future, sync::Arc};

use groundhog_logic::services::TransferProgress;
use tokio::{sync::mpsc, task::JoinHandle};

/// How a UI operation aborts when it is cancelled or superseded.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A sink for progress reports, handed to operations that make progress.
pub type Progress = Arc<dyn Fn(TransferProgress) + Send + Sync>;

struct Running {
    _handle: AbortOnDrop,
    /// Distinguishes this run from a later one under the same key.
    generation: u64,
    progress: Option<TransferProgress>,
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
    /// Start `future` under `key`, replacing (and aborting) any operator
    /// already running under it. When it finishes, `done(ticket, output)` is
    /// sent to `sender`, into the owning feature's event queue. Requires an
    /// async runtime context (the application runs inside one).
    pub fn start<T, E>(
        &mut self,
        key: K,
        future: impl Future<Output = T> + Send + 'static,
        sender: mpsc::UnboundedSender<E>,
        done: impl FnOnce(Ticket, T) -> E + Send + 'static,
    ) -> Ticket
    where
        T: Send + 'static,
        E: Send + 'static,
    {
        let ticket = self.ticket();
        let handle = tokio::spawn(async move {
            let _ = sender.send(done(ticket, future.await));
        });
        self.insert(key, handle, ticket);
        ticket
    }

    /// Start an operation that reports progress. `operation` receives the
    /// [`Progress`] sink; each report is sent to `sender` as
    /// `progress(ticket, p)` and the outcome as `done(ticket, output)`.
    pub fn start_with_progress<T, E, Fut>(
        &mut self,
        key: K,
        operation: impl FnOnce(Progress) -> Fut + Send + 'static,
        sender: mpsc::UnboundedSender<E>,
        progress: impl Fn(Ticket, TransferProgress) -> E + Send + 'static,
        done: impl FnOnce(Ticket, T) -> E + Send + 'static,
    ) -> Ticket
    where
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
        E: Send + 'static,
    {
        let ticket = self.ticket();
        let reports = sender.clone();
        let handle = tokio::spawn(async move {
            // The sink and the channel are dropped with the operation's
            // future, which ends the progress stream when the operation ends
            // or the task is aborted.
            let (report_tx, mut report_rx) = mpsc::unbounded_channel();
            let sink: Progress = Arc::new(move |report| {
                let _ = report_tx.send(report);
            });

            let operation = operation(sink);
            tokio::pin!(operation);

            loop {
                // Progress first: a report that arrives together with the
                // result must not be dropped after the loop breaks.
                tokio::select! {
                    biased;

                    report = report_rx.recv() => {
                        if let Some(report) = report {
                            let _ = reports.send(progress(ticket, report));
                        }
                    }

                    output = &mut operation => {
                        // Reports that raced the result still flow: drain
                        // them before the outcome, so the owner sees the
                        // same order the iced frontend's merged stream
                        // delivered.
                        while let Ok(report) = report_rx.try_recv() {
                            let _ = reports.send(progress(ticket, report));
                        }
                        let _ = sender.send(done(ticket, output));
                        return;
                    }
                }
            }
        });
        self.insert(key, handle, ticket);
        ticket
    }

    fn ticket(&mut self) -> Ticket {
        self.next_generation += 1;
        Ticket(self.next_generation)
    }

    fn insert(&mut self, key: K, handle: JoinHandle<()>, ticket: Ticket) {
        self.running.insert(
            key,
            Running {
                _handle: AbortOnDrop(handle),
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
    pub fn report(&mut self, key: &K, ticket: Ticket, progress: TransferProgress) {
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
    pub fn progress(&self, key: &K) -> Option<TransferProgress> {
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
    use futures::future::pending;

    /// The superseded run's result must be droppable, and cancellation
    /// forgets the run entirely — same contract as the iced frontend's.
    #[tokio::test]
    async fn stale_results_are_rejected() {
        let mut operations = Operations::<u8>::default();
        let (sender, mut results) = mpsc::unbounded_channel::<u64>();

        operations.start(1, async { 10u16 }, sender.clone(), |ticket, output| {
            ticket.0 * u64::from(output)
        });
        let first = operations.ticket_of(&1).expect("the first run");
        operations.start(1, async { 20u16 }, sender.clone(), |ticket, output| {
            ticket.0 * u64::from(output)
        });
        let second = operations.ticket_of(&1).expect("the second run");

        // Let both tasks settle: the second finishes, the first is aborted.
        tokio::task::yield_now().await;

        assert!(
            !operations.finish(&1, first),
            "a superseded run's ticket is stale"
        );
        assert!(operations.is_running(&1));

        // The second run's result arrived and is accepted once.
        assert!(operations.finish(&1, second));
        assert!(!operations.is_running(&1));

        // A run is never finished twice, and the sender's value matched.
        assert!(!operations.finish(&1, second));
        assert_eq!(results.try_recv().ok(), Some(2 * 20), "second run's value");
    }

    #[tokio::test]
    async fn cancelling_forgets_the_run() {
        let mut operations = Operations::<u8>::default();
        let (sender, _results) = mpsc::unbounded_channel::<Torrent>();

        // A future that never resolves: aborting it is the only way out.
        operations.start(3, pending::<u8>(), sender, |ticket, _| ticket.0 as u32);
        let ticket = operations.ticket_of(&3).expect("the running operation");

        assert!(operations.cancel(&3));
        assert!(
            !operations.finish(&3, ticket),
            "a cancelled run's result is stale"
        );
        assert!(!operations.cancel(&3));
    }

    #[tokio::test]
    async fn progress_is_kept_per_run() {
        let mut operations = Operations::<u8>::default();
        let (sender, _results) = mpsc::unbounded_channel::<Torrent>();

        operations.start(1, async {}, sender, |ticket, _| ticket.0 as u32);
        let ticket = operations.ticket_of(&1).expect("the running operation");

        let report = TransferProgress { done: 2, total: 5 };
        operations.report(&1, Ticket(999), report);
        assert_eq!(operations.progress(&1), None, "stale report ignored");
        operations.report(&1, ticket, report);
        assert_eq!(operations.progress(&1), Some(report));
    }

    #[tokio::test]
    async fn progress_and_results_both_arrive() {
        let mut operations = Operations::<u8>::default();
        let (sender, mut outcomes) = mpsc::unbounded_channel::<Outcome>();

        operations.start_with_progress(
            7,
            |sink| async move {
                sink(TransferProgress { done: 3, total: 9 });
                42u16
            },
            sender,
            |ticket, progress| Outcome::Progress(ticket.0, progress),
            |ticket, output| Outcome::Done(ticket.0, output),
        );
        let ticket = operations.ticket_of(&7).expect("the running operation");

        // The operation completes on first poll; both events queue up.
        tokio::task::yield_now().await;

        let mut progress = None;
        let mut done = None;
        while let Ok(outcome) = outcomes.try_recv() {
            match outcome {
                Outcome::Progress(generation, report) => {
                    operations.report(&7, Ticket(generation), report);
                    progress = Some(operations.progress(&7).expect("its own report"));
                }
                Outcome::Done(generation, output) => {
                    assert!(operations.finish(&7, Ticket(generation)));
                    assert_eq!(output, 42);
                    done = Some(generation == ticket.0);
                }
            }
        }

        assert_eq!(progress, Some(TransferProgress { done: 3, total: 9 }));
        assert_eq!(done, Some(true));
        assert!(!operations.is_running(&7));
    }

    /// What `Operations::start_with_progress` sends in the tests: either a
    /// progress report or the outcome, each tagged with its ticket.
    #[derive(Debug, PartialEq)]
    enum Outcome {
        Progress(u64, TransferProgress),
        Done(u64, u16),
    }

    /// The type of an operation without a meaningful result, for signatures.
    type Torrent = u32;
}
