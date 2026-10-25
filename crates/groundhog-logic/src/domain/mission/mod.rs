//! The mission model.
//!
//! - [`MissionItem`]: one item in wire form, losslessly, with meaning layered
//!   on by the [`command`] table.
//! - [`MissionPlan`]: the editable list of items, with stable [`ItemId`]s.
//! - [`MissionDraft`]: the operator's plan for a vehicle, kept against what
//!   the vehicle holds.
//! - [`OnboardMission`]: what a vehicle reported having, as last transferred.
//! - [`codec`]: plan ↔ wire under a firmware's [`MissionLayout`].
//!
//! [`MissionLayout`]: crate::domain::firmware::MissionLayout

pub(crate) mod codec;
pub mod command;
mod draft;
mod item;
mod plan;

pub use draft::{Caution, MissionAction, MissionDraft, Refusal, SyncStatus, Verdict};
pub use item::{Field, FieldKind, FieldView, MissionItem, Position};
pub use plan::{ItemId, MissionPlan};

/// The default altitude of the first waypoint of a plan, above home.
pub const DEFAULT_WAYPOINT_ALTITUDE_M: f32 = 50.0;

/// A plan has more items than a mission can number with 16-bit wire
/// sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the plan has more items than a mission can number")]
pub struct PlanTooLong;

/// The mission a vehicle holds, as of the last successful transfer.
#[derive(Debug, Clone, PartialEq)]
pub struct OnboardMission {
    /// The plan items, home excluded.
    pub items: Vec<MissionItem>,
    /// The home slot's item, for firmwares that have one.
    pub home: Option<MissionItem>,
    /// The vehicle's id for this mission (`opaque_id`), or 0 if it has none.
    pub opaque_id: u32,
}

impl OnboardMission {
    /// An empty onboard mission, as after a clear.
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            home: None,
            opaque_id: 0,
        }
    }
}
