use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use mav_param::Ident;
use mavio::default_dialect::enums::MavCmd;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use super::lock;
use crate::foundation::MavlinkId;

/// Identifies a resource of which only a single transaction may be in flight
/// at any time, because the responses cannot be told apart.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockKey {
    /// A command of a specific kind (`MavCmd as u16`) to a specific target
    Command(MavlinkId, u16),
    /// Setting a single parameter on a specific target
    Param(MavlinkId, Ident),
    /// Downloading the full parameter list of a specific target
    ParamList(MavlinkId),
    /// A mission transfer (download/upload/clear) with a specific target and mission type.
    #[allow(dead_code)] // Consumed by tests until the mission UI lands
    Mission(MavlinkId, u8), // u8 = MavMissionType discriminant; the enum is not Eq/Ord
}

impl LockKey {
    pub fn command(target: MavlinkId, command: MavCmd) -> Self {
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
    pub async fn lock(&self, key: LockKey) -> KeyGuard {
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

    /// Acquire the lock for `key` only if nobody else holds or waits for it.
    pub fn try_lock(&self, key: LockKey) -> Option<KeyGuard> {
        let entry = self.entry(&key);

        // Construct KeyGuard before attempting lock to ensure cleanup on failure
        let mut key_guard = KeyGuard {
            guard: None,
            key,
            map: self.clone(),
        };

        key_guard.guard = Some(entry.try_lock_owned().ok()?);

        Some(key_guard)
    }

    /// Whether the lock for `key` is currently held or waited for.
    pub fn is_busy(&self, key: &LockKey) -> bool {
        lock(&self.map)
            .get(key)
            .is_some_and(|mutex| Arc::strong_count(mutex) > 1)
    }
}

/// Holds a [`LockKey`] until dropped.
pub struct KeyGuard {
    guard: Option<OwnedMutexGuard<()>>,
    map: LockMap,
    key: LockKey,
}

impl Drop for KeyGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        self.map.cleanup(&self.key);
    }
}
