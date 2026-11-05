//! Shared harness for feature tests.
//!
//! A feature test drives a panel's `update` with a [`Context`] over a
//! synthetic [`Fleet`] and a real [`FleetHandle`], then asserts on the
//! feature's state, its notices and the effects it requested. [`Env`] owns
//! the fleet and the context's inputs, and drives any [`Panel`].

use std::time::Instant;

use groundhog_logic::{
    Backend, BackendOptions, MavlinkId, SystemId,
    domain::fleet::{Fleet, FleetSnapshot},
    services::FleetHandle,
};
use iced::Task;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavModeFlag, MavType},
        messages::Heartbeat,
    },
};

use super::{Context, Effect, Panel, ViewContext};
use crate::{config::Settings, features::notifications::Notifications};

/// A synthetic fleet and everything a feature's `update` reads from.
pub struct Env {
    pub fleet: Fleet,
    /// A running backend, for its handle and its links. Its fleet is not
    /// `fleet`: tests fold messages into `fleet` themselves.
    pub backend: Backend,
    pub handle: FleetHandle,
    pub notices: Notifications,
    pub settings: Settings,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// An environment with no vehicle yet; [`Env::observe`] adds one.
    pub fn new() -> Self {
        let (backend, _events) = Backend::start(BackendOptions {
            bootstrap: false,
            ..BackendOptions::default()
        });

        Self {
            fleet: Fleet::default(),
            handle: backend.fleet().clone(),
            backend,
            notices: Notifications::default(),
            settings: Settings::default(),
        }
    }

    /// Fold one message from `source` into the fleet, as the runtime would.
    pub fn observe(&mut self, source: MavlinkId, message: &DefaultDialect) {
        self.fleet.observe(source, Instant::now(), Some(message));
    }

    /// Add an ArduPilot copter at `source`, in `custom_mode`, armed or not.
    pub fn observe_vehicle(&mut self, source: MavlinkId, armed: bool, custom_mode: u32) {
        let mut base_mode = MavModeFlag::CUSTOM_MODE_ENABLED;
        if armed {
            base_mode |= MavModeFlag::SAFETY_ARMED;
        }

        self.observe(
            source,
            &DefaultDialect::Heartbeat(Heartbeat {
                type_: MavType::Quadrotor,
                autopilot: MavAutopilot::Ardupilotmega,
                base_mode,
                custom_mode,
                ..Default::default()
            }),
        );
    }

    /// Run one message through `feature`'s `update`, returning the effects it
    /// requested. The task it returned is dropped, as a view interaction
    /// whose result the test does not drive.
    pub fn step<P: Panel>(
        &mut self,
        panel: &mut P,
        message: P::Message,
    ) -> Vec<Effect<P::Message>> {
        let (task, effects) = self.update(panel, message);
        drop(task);
        effects
    }

    /// Run one message through `panel`'s `update`, returning the task it
    /// returned and the effects it requested.
    pub fn update<P: Panel>(
        &mut self,
        panel: &mut P,
        message: P::Message,
    ) -> (Task<P::Message>, Vec<Effect<P::Message>>) {
        self.with_context(|cx| panel.update(message, cx))
    }

    /// Tell `panel` the fleet published, as the application does with each
    /// snapshot. Returns the effects it requested.
    pub fn on_fleet<P: Panel>(&mut self, panel: &mut P) -> Vec<Effect<P::Message>> {
        let (task, effects) = self.with_context(|cx| panel.on_fleet(cx));
        drop(task);
        effects
    }

    /// Tell `panel` the time is `now`, as the application does while it
    /// wants ticks. Returns the effects it requested.
    pub fn on_tick<P: Panel>(&mut self, panel: &mut P, now: Instant) -> Vec<Effect<P::Message>> {
        let (task, effects) = self.with_context(|cx| panel.on_tick(now, cx));
        drop(task);
        effects
    }

    /// What a view of `fleet` reads, with `selected` shown.
    pub fn view_context<'a>(
        &'a self,
        fleet: &'a FleetSnapshot,
        selected: Option<SystemId>,
    ) -> ViewContext<'a> {
        ViewContext {
            fleet,
            selected,
            following: false,
            notices: &self.notices,
            links: self.backend.links(),
            settings: &self.settings,
            recorder: None,
            recordings: None,
        }
    }

    fn with_context<M>(
        &mut self,
        run: impl FnOnce(&mut Context<'_, M>) -> Task<M>,
    ) -> (Task<M>, Vec<Effect<M>>) {
        let snapshot = self.fleet.snapshot();
        let mut cx = Context::new(
            &snapshot,
            &self.handle,
            self.backend.links(),
            &mut self.notices,
            None,
            &self.settings,
        );
        let task = run(&mut cx);
        (task, cx.into_effects())
    }
}
