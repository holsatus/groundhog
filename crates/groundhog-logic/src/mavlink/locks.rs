//! Per-resource locks that serialise transactions.
//!
//! MAVLink replies carry no transaction id, so two transactions on the same
//! resource (a command kind, a parameter, a mission type, on one target)
//! would take each other's replies. Each takes the [`LockKey`] for its
//! resource first; waiters are served in order, and a lock no one holds or
//! waits for is removed.
//!
//! One resource has two modes: a target's parameter reply channel. A list
//! download reads every `PARAM_VALUE` from the target and so holds it
//! exclusively; a set only waits for one parameter's echo and holds it shared,
//! so sets of different parameters still run side by side. A set also takes
//! the key for its own parameter, which serialises sets of the same one.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use mav_param::Ident;
use mavio::default_dialect::enums::MavCmd;
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
};

use super::lock;
use crate::MavlinkId;

/// Identifies a resource of which only a single transaction may be in flight
/// at any time, because the responses cannot be told apart.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LockKey {
    /// A command of a specific kind (`MavCmd as u16`) to a specific target
    Command(MavlinkId, u16),
    /// Setting a single parameter on a specific target
    Param(MavlinkId, Ident),
    /// A target's parameter reply channel: a list download holds it
    /// exclusively, a set shares it.
    ParamChannel(MavlinkId),
    /// A mission transfer (download/upload/clear) with a specific target and mission type.
    Mission(MavlinkId, u8), // u8 = MavMissionType discriminant; the enum is not Eq/Ord
}

impl LockKey {
    pub(super) fn command(target: MavlinkId, command: MavCmd) -> Self {
        LockKey::Command(target, command as u16)
    }
}

/// A set of FIFO-fair async mutexes, created on demand for each [`LockKey`].
///
/// Exclusive keys use a mutex; [`LockKey::ParamChannel`] uses a reader-writer
/// lock so a download can exclude sets while sets share.
#[derive(Default, Clone)]
pub(super) struct LockMap {
    exclusive: Arc<Mutex<BTreeMap<LockKey, Arc<AsyncMutex<()>>>>>,
    channels: Arc<Mutex<BTreeMap<LockKey, Arc<RwLock<()>>>>>,
}

impl LockMap {
    fn exclusive_entry(&self, key: &LockKey) -> Arc<AsyncMutex<()>> {
        let mut map = lock(&self.exclusive);
        // Opportunistically clean out entries that nobody holds or waits for
        map.retain(|_, mutex| Arc::strong_count(mutex) > 1);
        map.entry(key.clone()).or_default().clone()
    }

    fn channel_entry(&self, key: &LockKey) -> Arc<RwLock<()>> {
        let mut map = lock(&self.channels);
        map.retain(|_, lock| Arc::strong_count(lock) > 1);
        map.entry(key.clone()).or_default().clone()
    }

    fn cleanup(&self, key: &LockKey) {
        {
            let mut map = lock(&self.exclusive);
            if map
                .get(key)
                .is_some_and(|mutex| Arc::strong_count(mutex) == 1)
            {
                map.remove(key);
            }
        }

        let mut map = lock(&self.channels);
        if map
            .get(key)
            .is_some_and(|channel| Arc::strong_count(channel) == 1)
        {
            map.remove(key);
        }
    }

    /// Acquire the exclusive lock for `key`, waiting in line if it is held.
    /// This is the write side for [`LockKey::ParamChannel`].
    pub(super) async fn lock(&self, key: LockKey) -> KeyGuard {
        let entry = self.exclusive_entry(&key);

        // Construct KeyGuard before awaiting to ensure cleanup on canceled future
        let mut key_guard = KeyGuard {
            guard: None,
            key,
            map: self.clone(),
        };

        key_guard.guard = Some(Held::Exclusive(entry.lock_owned().await));

        key_guard
    }

    /// Take the write side of `target`'s parameter reply channel: a list
    /// download excludes every set while it runs.
    pub(super) async fn lock_channel(&self, target: MavlinkId) -> KeyGuard {
        let key = LockKey::ParamChannel(target);
        let entry = self.channel_entry(&key);

        let mut key_guard = KeyGuard {
            guard: None,
            key,
            map: self.clone(),
        };

        key_guard.guard = Some(Held::Channel(entry.write_owned().await));

        key_guard
    }

    /// Take the shared side of `target`'s parameter reply channel, letting
    /// other sets run but keeping a download out.
    pub(super) async fn lock_channel_shared(&self, target: MavlinkId) -> KeyGuard {
        let key = LockKey::ParamChannel(target);
        let entry = self.channel_entry(&key);

        let mut key_guard = KeyGuard {
            guard: None,
            key,
            map: self.clone(),
        };

        key_guard.guard = Some(Held::Shared(entry.read_owned().await));

        key_guard
    }

    /// Whether the lock for `key` is currently held or waited for.
    #[cfg(test)] // Reached only through the router's test-only `is_busy`
    pub(super) fn is_busy(&self, key: &LockKey) -> bool {
        lock(&self.exclusive)
            .get(key)
            .is_some_and(|mutex| Arc::strong_count(mutex) > 1)
            || lock(&self.channels)
                .get(key)
                .is_some_and(|channel| Arc::strong_count(channel) > 1)
    }
}

/// Holds a [`LockKey`] until dropped.
pub(super) struct KeyGuard {
    guard: Option<Held>,
    map: LockMap,
    key: LockKey,
}

#[expect(
    dead_code,
    reason = "the guards hold the lock; their fields are never read"
)]
enum Held {
    Exclusive(OwnedMutexGuard<()>),
    Shared(OwnedRwLockReadGuard<()>),
    Channel(OwnedRwLockWriteGuard<()>),
}

impl KeyGuard {
    /// Whether this guard is the lock for `key`.
    pub(super) fn covers(&self, key: &LockKey) -> bool {
        self.key == *key
    }
}

impl Drop for KeyGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        self.map.cleanup(&self.key);
    }
}
