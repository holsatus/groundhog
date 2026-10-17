//! Confirmation of consequential actions.
//!
//! Commands that change what a vehicle does (arming, starting a mission,
//! jumping to an item, clearing its mission) are never sent on a single
//! click. The feature asks for a [`Confirmation`]; the application shows it
//! as a modal dialog and performs the confirmation's [`Action`] only if the
//! operator accepts.
//!
//! In iced the confirmation carried a message; in immediate mode it carries
//! an action enum instead, which the application dispatches back to the
//! owning feature. The enum is deliberately small: only the consequential
//! vehicle commands listed in the design decisions may appear.

use groundhog_logic::{
    SystemId,
    domain::{firmware::ModeKind, mission::ItemId},
};

use super::mission::{MissionOp, PlanKey};

/// The operator's answer.
#[derive(Debug, Clone, Copy)]
pub enum Answer {
    Accept,
    Dismiss,
}

/// A consequential vehicle command selected from the confirmation dialog.
#[derive(Debug, Clone)]
pub enum Action {
    /// Arm (`true`) or disarm.
    Arm { system: SystemId, arm: bool },
    /// Command the vehicle's mode of `kind` (return, land, ...).
    ModeKind { system: SystemId, kind: ModeKind },
    Mission {
        plan: PlanKey,
        op: MissionOp,
        item: Option<ItemId>,
    },
}

/// A question to put to the operator, and the action to run on "yes".
#[derive(Debug, Clone)]
pub struct Confirmation {
    pub title: String,
    pub body: String,
    pub accept_label: &'static str,
    /// Styles the accept button as dangerous.
    pub danger: bool,
    pub action: Action,
}

impl Confirmation {
    pub fn new(title: impl Into<String>, body: impl Into<String>, action: Action) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            accept_label: "Confirm",
            danger: false,
            action,
        }
    }

    pub fn accept_label(mut self, label: &'static str) -> Self {
        self.accept_label = label;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}
