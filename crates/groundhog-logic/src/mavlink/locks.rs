//! Per-resource locks that serialise transactions.
//!
//! MAVLink replies carry no transaction id, so two transactions on the same
//! resource (a command kind, a parameter, a mission type, on one target)
//! would take each other's replies. Each takes the [`LockKey`] for its
//! resource first; waiters are served in order, and a lock no one holds or
//! waits for is removed.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use mav_param::Ident;
use mavio::default_dialect::enums::MavCmd;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

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
    /// Downloading the full parameter list of a specific target
    ParamList(MavlinkId),
    /// A mission transfer (download/upload/clear) with a specific target and mission type.
    Mission(MavlinkId, u8), // u8 = MavMissionType discriminant; the enum is not Eq/Ord
}

impl LockKey {
    pub(super) fn command(target: MavlinkId, command: MavCmd) -> Self {
        LockKey::Command(target, command as u16)
    }
}

/// A set of FIFO-fair async mutexes, created on demand for each [`LockKey`].
#[derive(Default, Clone)]
pub(super) struct LockMap {
    map: Arc<Mutex<BTreeMap<LockKey, Arc<AsyncMutex<()>>>>>,
}

impl LockMap {
    fn entry(&self, key: &LockKey) -> Arc<AsyncMutex<()>> {
        let mut map = lock(&self.map);
        // Opportunistically clean out entries that nobody holds or waits for
        map.retain(|_, mutex| Arc::strong_count(mutex) > 1);
        map.entry(key.clone()).or_default().clone()
    }

    fn cleanup(&self, key: &LockKey) {
        let mut map = lock(&self.map);
        if map
            .get(key)
            .is_some_and(|mutex| Arc::strong_count(mutex) == 1)
        {
            map.remove(key);
        }
    }

    /// Acquire the lock for `key`, waiting in line if it is already held.
    pub(super) async fn lock(&self, key: LockKey) -> KeyGuard {
        let entry = self.entry(&key);

        // Construct KeyGuard before awaiting to ensure cleanup on canceled future
        let mut key_guard = KeyGuard {
            guard: None,
            key,
            map: self.clone(),
        };

        key_guard.guard = Some(entry.lock_owned().await);

        key_guard
    }

    /// Whether the lock for `key` is currently held or waited for.
    #[cfg(test)] // Reached only through the router's test-only `is_busy`
    pub(super) fn is_busy(&self, key: &LockKey) -> bool {
        lock(&self.map)
            .get(key)
            .is_some_and(|mutex| Arc::strong_count(mutex) > 1)
    }
}

/// Holds a [`LockKey`] until dropped.
pub(super) struct KeyGuard {
    guard: Option<OwnedMutexGuard<()>>,
    map: LockMap,
    key: LockKey,
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
