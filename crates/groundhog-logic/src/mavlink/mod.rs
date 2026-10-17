//! MAVLink middleware: routes messages between links and vehicles, and provides
//! ergonomic async transactions (commands, message requests, parameters) on top.
//!
//! [`MavlinkRouter`] lives for the lifetime of the application, and is cheap to
//! clone into async tasks. Links are attached to it, all inbound messages are
//! delivered to the application as [`Event`]s, and transactions are run against
//! a target by passing its [`MavlinkId`].

use std::sync::{Mutex, MutexGuard, PoisonError};

use mavio::default_dialect::messages::{
    CommandCancel, CommandInt, CommandLong, MissionAck, MissionClearAll, MissionCount,
    MissionItemInt, MissionRequestInt, MissionRequestList, MissionSetCurrent, ParamRequestList,
    ParamRequestRead, ParamSet,
};

use crate::MavlinkId;

mod acks;
pub mod commands;
mod dedup;
pub(crate) mod error;
pub(crate) mod io;
mod locks;
mod mission;
mod param;
mod recv;
mod router;
mod transaction;

pub use error::MavError;
pub use mission::TransferProgress;
#[cfg(test)]
pub use router::LinkGuard;
pub use router::{Event, EventReceiver, LinkStats, MavlinkRouter};
pub use transaction::Command;

/// The GCS's default MAVLink identity, stamped on outbound frames.
pub const DEFAULT_GCS_ID: MavlinkId = MavlinkId {
    system: 255,
    component: 1,
};

#[cfg(test)]
mod tests;

/// Lock a std mutex, ignoring poisoning. The protected data is always left in
/// a consistent state, since no locks are held across panicking code.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a message from `source` matches the `target`, where a target
/// component of 0 matches any component of the system.
fn matches_target(target: MavlinkId, source: MavlinkId) -> bool {
    target.system == source.system
        && (target.component == 0 || target.component == source.component)
}

/// A message with `target_system` and `target_component` fields.
pub trait Targeted: mavio::Message {
    fn set_target(&mut self, target: MavlinkId);
}

macro_rules! impl_targeted {
    ($($message:ty),* $(,)?) => {$(
        impl Targeted for $message {
            fn set_target(&mut self, target: MavlinkId) {
                self.target_system = target.system;
                self.target_component = target.component;
            }
        }
    )*};
}

impl_targeted!(
    CommandCancel,
    CommandInt,
    CommandLong,
    MissionAck,
    MissionClearAll,
    MissionCount,
    MissionItemInt,
    MissionRequestInt,
    MissionRequestList,
    MissionSetCurrent,
    ParamRequestList,
    ParamRequestRead,
    ParamSet,
);
