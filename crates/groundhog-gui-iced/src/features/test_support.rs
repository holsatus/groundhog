//! Shared harness for feature tests.
//!
//! A feature test drives a panel's `update` with a [`Context`] over a
//! synthetic [`Fleet`] and a real [`FleetHandle`], then asserts on the
//! feature's state, its notices and the effects it requested. [`Env`] owns
//! the fleet and the context's inputs; [`Feature`] adapts each panel's
//! `update` to [`Env::step`].

use std::time::Instant;

use groundhog_logic::{
    Backend, BackendOptions, MavlinkId, domain::fleet::Fleet, services::FleetHandle,
};
use iced::Task;
use mavio::{
    DefaultDialect,
    default_dialect::{
        enums::{MavAutopilot, MavModeFlag, MavType},
        messages::Heartbeat,
    },
};

use super::{
    Context, Effect,
    calibration::{CalibrationPanel, Message as CalibrationMessage},
    link_panel::{LinkPanel, Message as LinkMessage},
    mission::{Message as MissionMessage, MissionEditor},
    parameter_panel::{Message as ParameterMessage, ParameterPanel},
    vehicles::{Message as VehicleMessage, VehiclePanel},
    video::{Message as VideoMessage, VideoPanel},
};
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
    pub fn step<M, F: Feature<M>>(&mut self, feature: &mut F, message: M) -> Vec<Effect<M>> {
        run(self, feature, message)
    }

    /// Run one message through `feature`'s `update`, returning the task it
    /// returned and the effects it requested.
    pub fn update<M, F: Feature<M>>(
        &mut self,
        feature: &mut F,
        message: M,
    ) -> (Task<M>, Vec<Effect<M>>) {
        let snapshot = self.fleet.snapshot();
        let mut cx = Context::new(
            &snapshot,
            &self.handle,
            self.backend.links_mut(),
            &mut self.notices,
            None,
            &self.settings,
        );
        let task = feature.update(message, &mut cx);
        (task, cx.into_effects())
    }
}

/// [`Env::step`] as a function, for a test module that wraps the message in
/// one of its own.
pub fn run<M, F: Feature<M>>(env: &mut Env, feature: &mut F, message: M) -> Vec<Effect<M>> {
    let (task, effects) = env.update(feature, message);
    drop(task);
    effects
}

/// A panel whose `update` a test can drive through [`Env::step`].
pub trait Feature<M> {
    fn update(&mut self, message: M, cx: &mut Context<M>) -> Task<M>;
}

impl Feature<CalibrationMessage> for CalibrationPanel {
    fn update(
        &mut self,
        message: CalibrationMessage,
        cx: &mut Context<CalibrationMessage>,
    ) -> Task<CalibrationMessage> {
        CalibrationPanel::update(self, message, cx)
    }
}

impl Feature<MissionMessage> for MissionEditor {
    fn update(
        &mut self,
        message: MissionMessage,
        cx: &mut Context<MissionMessage>,
    ) -> Task<MissionMessage> {
        MissionEditor::update(self, message, cx)
    }
}

impl Feature<ParameterMessage> for ParameterPanel {
    fn update(
        &mut self,
        message: ParameterMessage,
        cx: &mut Context<ParameterMessage>,
    ) -> Task<ParameterMessage> {
        ParameterPanel::update(self, message, cx)
    }
}

impl Feature<VehicleMessage> for VehiclePanel {
    fn update(
        &mut self,
        message: VehicleMessage,
        cx: &mut Context<VehicleMessage>,
    ) -> Task<VehicleMessage> {
        VehiclePanel::update(self, message, cx)
    }
}

impl Feature<LinkMessage> for LinkPanel {
    fn update(&mut self, message: LinkMessage, cx: &mut Context<LinkMessage>) -> Task<LinkMessage> {
        LinkPanel::update(self, message, cx)
    }
}

impl Feature<VideoMessage> for VideoPanel {
    fn update(
        &mut self,
        message: VideoMessage,
        cx: &mut Context<VideoMessage>,
    ) -> Task<VideoMessage> {
        VideoPanel::update(self, message, cx)
    }
}
