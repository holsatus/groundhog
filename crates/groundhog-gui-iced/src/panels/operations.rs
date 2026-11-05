//! Tracking of long-running operations started from the UI.
//!
//! An [`Operations`] set owns the abort handle of every operation it started,
//! keyed by what the operation is about (a vehicle and an operation kind, a
//! parameter, ...). That gives each panel, for free:
//! - "is this running?" per key, to disable buttons and show spinners;
//! - cancellation: [`Operations::cancel`] aborts the task, which drops the
//!   transaction's future and lets the middleware clean up (release locks,
//!   cancel commands and mission transfers on the vehicle);
//! - progress, for operations that report it;
//! - stale-result filtering: a result whose operation was cancelled or
//!   superseded is dropped by [`Operations::settle`];
//! - the operator hears of every failure: [`Operations::settle`] pushes a
//!   notice worded from the key's [`Activity`], the same way in every
//!   panel.
//!
//! A panel routes the [`Finished`] (and [`Progressed`]) messages its
//! operations end with back into its set, and handles what [`Settled`] says:
//! the success value, and whatever a failure changes besides the notice.

use std::{borrow::Cow, collections::BTreeMap, fmt, future::Future, sync::Arc};

use futures::{StreamExt, channel::mpsc, stream};
use iced::Task;

use groundhog_logic::{
    Progress, SystemId,
    domain::fleet::FleetSnapshot,
    services::{Error, Result},
};

use super::{Context, notifications::Notice};

/// A sink for progress reports, handed to operations that make progress.
pub type ProgressReporter = Arc<dyn Fn(Progress) + Send + Sync>;

/// What a run under a key does, for the notice when it fails. `C` is the
/// context each run carries beside its key (what a parameter upload sets,
/// whether tracking starts or stops), `()` when the key says it all.
pub trait Activity<C = ()> {
    /// What the run does, as the start of a sentence: "Arming", "Setting
    /// RATE_P", "Mission upload".
    fn activity(&self, context: &C) -> Cow<'static, str>;

    /// The vehicle the run concerns.
    fn system(&self) -> SystemId;

    /// Whether a failure with `error` is news to the operator. Some are
    /// announced another way (a calibration that fails to start ends its
    /// session, which says so).
    fn announces(&self, _context: &C, _error: &Error) -> bool {
        true
    }

    /// The notice for the run failing with `detail`.
    fn failure(&self, context: &C, detail: impl ToString, fleet: &FleetSnapshot) -> Notice {
        let system = self.system();
        Notice::error(format!(
            "{} on {} failed",
            self.activity(context),
            fleet.vehicle_name(system)
        ))
        .detail(detail)
        .vehicle(system)
    }
}

/// Identifies one run of an operation, so its result can be told from a
/// later run's under the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

/// An operation's result, as the message it ends with.
#[derive(Clone)]
pub struct Finished<K, T> {
    key: K,
    ticket: Ticket,
    result: Result<T>,
}

/// A summary: results can be large (a whole mission), and messages are
/// formatted for diagnostics on the UI thread.
impl<K: fmt::Debug, T> fmt::Debug for Finished<K, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Finished")
            .field("key", &self.key)
            .field("ok", &self.result.is_ok())
            .finish()
    }
}

/// A progress report of a running operation, as a message.
#[derive(Debug, Clone)]
pub struct Progressed<K> {
    key: K,
    ticket: Ticket,
    progress: Progress,
}

/// A result that was current, after [`Operations::settle`] announced its
/// failure, if it failed.
pub struct Settled<K, C, T> {
    pub key: K,
    pub context: C,
    pub result: Result<T>,
}

struct Running<C> {
    /// Aborts the task when dropped.
    _handle: iced::task::Handle,
    ticket: Ticket,
    context: C,
    progress: Option<Progress>,
}

/// The operations a panel has in flight, by key.
pub struct Operations<K, C = ()> {
    running: BTreeMap<K, Running<C>>,
    next_ticket: u64,
}

impl<K, C> Default for Operations<K, C> {
    fn default() -> Self {
        Self {
            running: BTreeMap::new(),
            next_ticket: 0,
        }
    }
}

impl<K: Ord + Clone + Send + 'static> Operations<K> {
    /// Start `future` under `key`, replacing (and aborting) any operation
    /// already running under it. `done` wraps its result in the panel's
    /// message, to be handed to [`Operations::settle`].
    pub fn start<T, M>(
        &mut self,
        key: K,
        future: impl Future<Output = Result<T>> + Send + 'static,
        done: impl FnOnce(Finished<K, T>) -> M + Send + 'static,
    ) -> Task<M>
    where
        T: Send + 'static,
        M: Send + 'static,
    {
        self.start_with(key, (), future, done)
    }

    /// Start an operation that reports progress. `operation` receives the
    /// [`ProgressReporter`] sink; `progress` wraps each report in the
    /// panel's message, to be handed to [`Operations::report`].
    pub fn start_with_progress<T, M, Fut>(
        &mut self,
        key: K,
        operation: impl FnOnce(ProgressReporter) -> Fut,
        progress: impl Fn(Progressed<K>) -> M + Send + 'static,
        done: impl FnOnce(Finished<K, T>) -> M + Send + 'static,
    ) -> Task<M>
    where
        Fut: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
        M: Send + 'static,
    {
        let ticket = self.ticket();
        let (sender, receiver) = mpsc::unbounded();
        let sink: ProgressReporter = Arc::new(move |report| {
            drop(sender.unbounded_send(report));
        });

        // The sink is dropped with the operation's future, which ends the
        // progress stream, so the merged stream ends with the operation.
        let future = operation(sink);
        let reported = key.clone();
        let reports = receiver.map(move |report| {
            Some(progress(Progressed {
                key: reported.clone(),
                ticket,
                progress: report,
            }))
        });
        let finished = key.clone();
        let mut done = Some(done);
        let result = stream::once(future).map(move |result| {
            done.take().map(|done| {
                done(Finished {
                    key: finished.clone(),
                    ticket,
                    result,
                })
            })
        });

        let merged = stream::select(reports, result).filter_map(futures::future::ready);

        let (task, handle) = Task::stream(merged).abortable();
        self.insert(key, handle, ticket, ());
        task
    }
}

impl<K: Ord + Clone + Send + 'static, C> Operations<K, C> {
    /// Start `future` under `key` with the run's `context`, replacing (and
    /// aborting) any operation already running under it. The context comes
    /// back with the result from [`Operations::settle`].
    pub fn start_with<T, M>(
        &mut self,
        key: K,
        context: C,
        future: impl Future<Output = Result<T>> + Send + 'static,
        done: impl FnOnce(Finished<K, T>) -> M + Send + 'static,
    ) -> Task<M>
    where
        T: Send + 'static,
        M: Send + 'static,
    {
        let ticket = self.ticket();
        let finished = key.clone();
        let (task, handle) = Task::perform(future, move |result| {
            done(Finished {
                key: finished,
                ticket,
                result,
            })
        })
        .abortable();
        self.insert(key, handle, ticket, context);
        task
    }

    fn ticket(&mut self) -> Ticket {
        self.next_ticket += 1;
        Ticket(self.next_ticket)
    }

    fn insert(&mut self, key: K, handle: iced::task::Handle, ticket: Ticket, context: C) {
        self.running.insert(
            key,
            Running {
                _handle: handle.abort_on_drop(),
                ticket,
                context,
                progress: None,
            },
        );
    }

    /// Take an operation's result. A stale one (its run was cancelled or
    /// superseded) is dropped: `None`. A current one ends its run; if it
    /// failed, the operator is told (unless the key's [`Activity`] says the
    /// failure announces itself), and the result is returned for whatever
    /// else it changes.
    pub fn settle<T, M>(
        &mut self,
        finished: Finished<K, T>,
        cx: &mut Context<M>,
    ) -> Option<Settled<K, C, T>>
    where
        K: Activity<C>,
    {
        let Finished {
            key,
            ticket,
            result,
        } = finished;
        if !self.is_current(&key, ticket) {
            return None;
        }
        let running = self.running.remove(&key)?;

        if let Err(error) = &result
            && key.announces(&running.context, error)
        {
            cx.notices
                .push(key.failure(&running.context, error, cx.fleet));
        }

        Some(Settled {
            key,
            context: running.context,
            result,
        })
    }

    fn is_current(&self, key: &K, ticket: Ticket) -> bool {
        self.running
            .get(key)
            .is_some_and(|running| running.ticket == ticket)
    }

    /// Record a progress report. Stale reports are ignored.
    pub fn report(&mut self, progressed: Progressed<K>) {
        if let Some(running) = self.running.get_mut(&progressed.key)
            && running.ticket == progressed.ticket
        {
            running.progress = Some(progressed.progress);
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

    /// Whether any key matching `matches` is running.
    pub fn any(&self, matches: impl Fn(&K) -> bool) -> bool {
        self.running.keys().any(matches)
    }

    /// The message the operation running under `key` would end with, had it
    /// ended with `result`: for tests that simulate results.
    #[cfg(test)]
    pub fn finished<T>(&self, key: &K, result: Result<T>) -> Finished<K, T> {
        let running = self.running.get(key).expect("the operation runs");
        Finished {
            key: key.clone(),
            ticket: running.ticket,
            result,
        }
    }

    /// The progress message the operation running under `key` would send,
    /// for tests that simulate progress.
    #[cfg(test)]
    pub fn progressed(&self, key: &K, progress: Progress) -> Progressed<K> {
        let running = self.running.get(key).expect("the operation runs");
        Progressed {
            key: key.clone(),
            ticket: running.ticket,
            progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use groundhog_logic::MavlinkId;

    use super::*;
    use crate::panels::test_support::Env;

    const SYSTEM: SystemId = SystemId(1);

    /// A key whose runs carry what they test as their context.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct Key(u8);

    impl Activity<&'static str> for Key {
        fn activity(&self, context: &&'static str) -> Cow<'static, str> {
            format!("Testing {context}").into()
        }

        fn system(&self) -> SystemId {
            SYSTEM
        }

        fn announces(&self, _context: &&'static str, error: &Error) -> bool {
            *error != Error::NotCalibrating
        }
    }

    fn env() -> Env {
        let mut env = Env::new();
        env.observe_vehicle(
            MavlinkId {
                system: SYSTEM.0,
                component: 1,
            },
            false,
            0,
        );
        env
    }

    /// Settle `finished` in a context over `env`.
    fn settle<T>(
        operations: &mut Operations<Key, &'static str>,
        env: &mut Env,
        finished: Finished<Key, T>,
    ) -> Option<Settled<Key, &'static str, T>> {
        let snapshot = env.fleet.snapshot();
        let mut cx = Context::<()>::new(
            &snapshot,
            &env.handle,
            env.backend.links(),
            &mut env.notices,
            None,
            &env.settings,
        );
        operations.settle(finished, &mut cx)
    }

    fn start(operations: &mut Operations<Key, &'static str>, key: u8, context: &'static str) {
        let _task = operations.start_with(Key(key), context, async { Ok(()) }, |finished| finished);
    }

    #[tokio::test]
    async fn a_superseded_or_cancelled_run_is_dropped() {
        let mut env = env();
        let mut operations = Operations::<Key, &'static str>::default();
        start(&mut operations, 1, "first");
        let first = operations.finished::<()>(&Key(1), Err(Error::Timeout));
        start(&mut operations, 1, "second");

        assert!(
            settle(&mut operations, &mut env, first).is_none(),
            "superseded"
        );
        assert!(operations.is_running(&Key(1)));
        assert_eq!(
            env.notices.history().count(),
            0,
            "a stale failure is not news"
        );

        let second = operations.finished(&Key(1), Ok(()));
        assert!(operations.cancel(&Key(1)));
        assert!(
            settle(&mut operations, &mut env, second).is_none(),
            "cancelled"
        );
    }

    #[tokio::test]
    async fn a_result_ends_its_run_and_brings_its_context_back() {
        let mut env = env();
        let mut operations = Operations::<Key, &'static str>::default();
        start(&mut operations, 1, "the context");
        let finished = operations.finished(&Key(1), Ok(7));

        let settled = settle(&mut operations, &mut env, finished).expect("current");

        assert_eq!(settled.key, Key(1));
        assert_eq!(settled.context, "the context");
        assert_eq!(settled.result, Ok(7));
        assert!(!operations.is_running(&Key(1)));
        assert_eq!(
            env.notices.history().count(),
            0,
            "a success is the panel's to tell"
        );
    }

    #[tokio::test]
    async fn a_failure_is_announced_in_one_wording() {
        let mut env = env();
        let mut operations = Operations::<Key, &'static str>::default();
        start(&mut operations, 1, "the link");
        let finished = operations.finished::<()>(&Key(1), Err(Error::Timeout));

        let settled = settle(&mut operations, &mut env, finished).expect("current");

        assert_eq!(
            settled.result,
            Err(Error::Timeout),
            "returned for what else it changes"
        );
        let notice = env.notices.history().next().expect("announced");
        let name = env.fleet.snapshot().vehicle_name(SYSTEM);
        assert_eq!(notice.title, format!("Testing the link on {name} failed"));
        assert_eq!(notice.detail, Some(Error::Timeout.to_string()));
        assert_eq!(notice.system, Some(SYSTEM));
    }

    #[tokio::test]
    async fn a_failure_that_announces_itself_is_not_repeated() {
        let mut env = env();
        let mut operations = Operations::<Key, &'static str>::default();
        start(&mut operations, 1, "quietly");
        let finished = operations.finished::<()>(&Key(1), Err(Error::NotCalibrating));

        let settled = settle(&mut operations, &mut env, finished).expect("current");

        assert_eq!(settled.result, Err(Error::NotCalibrating));
        assert_eq!(env.notices.history().count(), 0);
    }

    #[test]
    fn progress_is_kept_per_run() {
        let mut operations = Operations::<Key, &'static str>::default();
        start(&mut operations, 1, "first");
        let stale = operations.progressed(&Key(1), Progress { done: 1, total: 5 });
        start(&mut operations, 1, "second");

        operations.report(stale);
        assert_eq!(operations.progress(&Key(1)), None, "stale report ignored");

        let report = Progress { done: 2, total: 5 };
        operations.report(operations.progressed(&Key(1), report));
        assert_eq!(operations.progress(&Key(1)), Some(report));
    }
}
