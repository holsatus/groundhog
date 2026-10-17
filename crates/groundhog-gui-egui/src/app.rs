//! The application root: composition, event policy, and the docked panels.
//!
//! The root starts the backend at boot, keeps the latest fleet snapshot,
//! routes backend events into operator notices, and owns the dockable panel
//! layout. Vehicle state therefore keeps advancing off the UI thread while a
//! panel edits, exactly as the iced frontend's does.
//!
//! The frame loop mirrors iced's update/view split:
//! 1. drain backend events (snapshots, links, configuration writes);
//! 2. drain feature operation results into notices and plan updates;
//! 3. draw the dock, collecting effects and intents;
//! 4. apply what drawing collected.

use std::{path::Path, sync::Arc};

use egui_dock::{DockArea, DockState, NodeIndex, SurfaceIndex, TabViewer};
use mavio::default_dialect::enums::MavSeverity;

use groundhog_logic::{
    Backend, BackendOptions, SystemId,
    domain::{
        calibration::Phase as CalibrationPhase,
        fleet::{FleetEvent, FleetSnapshot},
        mission::SyncStatus,
        vehicle::{Liveness, status},
    },
    services::{FleetHandle, FleetUpdate, LinkEvent, LinkStatus, TlogRecorder},
};

use crate::{
    config::{ConfigWriter, Configuration},
    driver::{AppEvent, forward},
    features::{
        Context, Effect,
        calibration::CalibrationPanel,
        confirm::{Action, Answer, Confirmation},
        link_panel::LinkPanel,
        log_panel::LogPanel,
        mission::{MissionEditor, MissionIntent, PlanKey},
        notifications::{Level, Notice, Notifications},
        parameter_panel::ParameterPanel,
        vehicles::VehiclePanel,
    },
    map::{Map, MapIntent},
    style,
};

/// The panels, as the dock's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Map,
    Vehicles,
    Mission,
    Parameters,
    Calibration,
    Links,
    Logs,
    Settings,
}

impl Panel {
    fn title(self) -> &'static str {
        match self {
            Panel::Map => "Map",
            Panel::Vehicles => "Vehicles",
            Panel::Mission => "Mission",
            Panel::Parameters => "Parameters",
            Panel::Calibration => "Calibration",
            Panel::Links => "Links",
            Panel::Logs => "Logs",
            Panel::Settings => "Settings",
        }
    }

    /// The fixed map pane cannot be closed; everything else may.
    fn closable(self) -> bool {
        !matches!(self, Panel::Map)
    }

    /// The tabs the rail can toggle, in display order.
    pub const DOCKABLE: [Panel; 7] = [
        Panel::Vehicles,
        Panel::Mission,
        Panel::Parameters,
        Panel::Calibration,
        Panel::Links,
        Panel::Logs,
        Panel::Settings,
    ];
}

/// Everything the application owns.
pub struct Application {
    ctx: egui::Context,
    backend: Backend,
    services: FleetHandle,
    events: tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    config_writer: ConfigWriter,
    recorder: Option<TlogRecorder>,

    configuration: Configuration,
    dock: DockState<Panel>,
    map: Map,

    fleet: Arc<FleetSnapshot>,
    selected: Option<SystemId>,

    notices: Notifications,
    confirmation: Option<Confirmation>,
    /// The visuals last applied, so a settings change is applied on the next
    /// frame (iced applies them through its theme/scale callbacks).
    applied_visuals: (bool, f32),

    vehicles: VehiclePanel,
    mission: MissionEditor,
    parameters: ParameterPanel,
    calibration: CalibrationPanel,
    link_panel: LinkPanel,
    log_panel: LogPanel,
}

/// The initial dock layout: vehicles and mission on the left, the map fixed
/// in the centre, and the other tabs on the right.
fn initial_dock() -> DockState<Panel> {
    let mut dock = DockState::new(vec![Panel::Map]);
    let surface = dock.main_surface_mut();

    let [_, right] = surface.split_left(
        NodeIndex::root(),
        0.30,
        vec![Panel::Vehicles, Panel::Mission],
    );
    let [_, _tabs] = surface.split_left(
        right,
        0.70,
        vec![
            Panel::Parameters,
            Panel::Calibration,
            Panel::Links,
            Panel::Logs,
            Panel::Settings,
        ],
    );

    dock
}

impl Application {
    /// Start the backend and everything the UI drives. Called inside the
    /// async runtime, so the backend's tasks spawn where-upon the runtime's
    /// contexts are set.
    pub fn new(creation: &eframe::CreationContext<'_>) -> Self {
        let ctx = creation.egui_ctx.clone();

        let mut notices = Notifications::default();
        let configuration = Configuration::initialize()
            .inspect_err(|error| {
                notices.push(
                    Notice::error("Could not read the configuration").detail(error.to_string()),
                )
            })
            .unwrap_or_default();

        // The backend spawns its tasks here, inside the application's
        // runtime.
        let (backend, events) = Backend::start(BackendOptions {
            links: configuration.links.clone(),
            ..BackendOptions::default()
        });
        let services = backend.fleet().clone();

        let (config_writer, saves) = crate::config::spawn_writer();
        let events = forward(ctx.clone(), events, saves);

        let map = Map::new(ctx.clone());

        let mut application = Self {
            map,
            ctx,
            backend,
            services,
            events,
            config_writer,
            recorder: None,
            configuration: configuration.clone(),
            dock: initial_dock(),
            fleet: Arc::new(FleetSnapshot::default()),
            selected: None,
            notices,
            confirmation: None,
            applied_visuals: (
                configuration.settings.dark,
                configuration.settings.scale_factor,
            ),
            vehicles: VehiclePanel::new(),
            mission: MissionEditor::default(),
            parameters: ParameterPanel::new(),
            calibration: CalibrationPanel::default(),
            link_panel: LinkPanel::new(),
            log_panel: LogPanel::default(),
        };

        // One-time application theme: dark/light visuals and scale factor.
        application.apply_visuals();
        if application.configuration.settings.record_telemetry {
            application.start_recording();
        }
        application
    }

    /// One-time application theme: dark/light visuals and scale factor.
    fn apply_visuals(&mut self) {
        let settings = self.configuration.settings;
        self.applied_visuals = (settings.dark, settings.scale_factor);

        self.ctx.set_visuals(if settings.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        self.ctx.set_pixels_per_point(settings.scale_factor);
    }

    fn start_recording(&mut self) {
        let Some(directory) = crate::config::data_dir().map(|directory| directory.join("tlogs"))
        else {
            self.notices
                .push(Notice::error("No data directory to record telemetry into"));
            return;
        };

        match self.backend.record(&directory) {
            Ok(recorder) => self.recorder = Some(recorder),
            Err(error) => self.notices.push(
                Notice::error("Could not start telemetry recording").detail(error.to_string()),
            ),
        }
    }

    fn save_configuration(&mut self) {
        self.config_writer.save(self.configuration.clone());
    }

    /// Drain the backend's publications and feature results into the UI
    /// state and notices.
    fn drain(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                AppEvent::Fleet(update) => self.on_fleet(update),
                AppEvent::Link(event) => self.on_link(event),
                AppEvent::ConfigurationSaved(result) => {
                    if let Err(error) = result {
                        self.notices
                            .push(Notice::error("Could not save the configuration").detail(error));
                    }
                }
            }
        }

        // Feature results fold into the plan and become notices.
        let fleet = self.fleet.clone();
        let mut cx = Context::new(
            &fleet,
            &self.services,
            &mut self.notices,
            self.configuration.file_picker_path.as_deref(),
            &self.configuration.settings,
        );

        self.vehicles.collect(&mut cx);
        self.mission.collect(&mut cx);
        self.parameters.collect(&mut cx);
        self.calibration.collect(&mut cx);
        self.link_panel.collect_serial_ports(&mut cx);

        let effects = cx.into_effects();
        self.apply_effects(effects);

        self.notices.expire(std::time::Instant::now());
    }

    /// Fold a fleet publication in: replace the snapshot, sync drafts, and
    /// turn the events the runtime reported into notices.
    fn on_fleet(&mut self, update: FleetUpdate) {
        let FleetUpdate { snapshot, events } = update;

        self.mission.sync(&snapshot);
        self.fleet = snapshot;

        for event in events {
            self.on_fleet_event(event);
        }

        self.follow_selected();
    }

    /// Fleet events become selection changes and operator notices.
    fn on_fleet_event(&mut self, event: FleetEvent) {
        let fleet = self.fleet.clone();
        let name = |system| fleet.vehicle_name(system);

        let notice = match event {
            FleetEvent::Registered(system) => {
                if self.selected.is_none() {
                    self.selected = Some(system);
                }
                Notice::info(format!("{} connected", name(system))).vehicle(system)
            }
            // A late vehicle shows in the vehicle panel; only losing and
            // regaining contact are announced, so a flaky link does not
            // flood the operator.
            FleetEvent::LivenessChanged { system, from, to } => match (from, to) {
                (_, Liveness::Lost) => {
                    Notice::error(format!("Lost contact with {}", name(system))).vehicle(system)
                }
                (Liveness::Lost, Liveness::Active) => {
                    Notice::success(format!("Contact with {} restored", name(system)))
                        .vehicle(system)
                }
                _ => return,
            },
            FleetEvent::ArmedChanged(system, true) => {
                Notice::warning(format!("{} armed", name(system))).vehicle(system)
            }
            FleetEvent::ArmedChanged(system, false) => {
                Notice::info(format!("{} disarmed", name(system))).vehicle(system)
            }
            FleetEvent::ModeChanged(system, mode) => Notice::info(format!(
                "{}: {} mode",
                name(system),
                mode.map_or("unknown", |mode| mode.name)
            ))
            .vehicle(system),
            FleetEvent::StatusText(system, text) => {
                // Status texts are in the vehicle's own log; only the serious
                // ones are toasted, and not duplicated into the history.
                let level = match status::rank(text.severity) {
                    rank if rank <= status::rank(MavSeverity::Error) => Level::Error,
                    rank if rank <= status::rank(MavSeverity::Warning) => Level::Warning,
                    _ => return,
                };
                self.notices.toast(
                    Notice::new(level, format!("{}: {}", name(system), text.text)).vehicle(system),
                );
                return;
            }
            FleetEvent::OnboardMissionStale(system) => Notice::warning(format!(
                "The mission on {} was changed elsewhere",
                name(system)
            ))
            .detail("Download it to see the vehicle's current mission.")
            .vehicle(system),
            FleetEvent::CalibrationFinished(system, kind, phase) => {
                let subject = format!("{kind} calibration of {}", name(system));
                match phase {
                    CalibrationPhase::Succeeded => Notice::success(format!("{subject} succeeded")),
                    CalibrationPhase::Failed(reason) => {
                        Notice::error(format!("{subject} failed")).detail(reason)
                    }
                    CalibrationPhase::Cancelled => Notice::info(format!("{subject} cancelled")),
                    CalibrationPhase::Starting | CalibrationPhase::Running => return,
                }
                .vehicle(system)
            }
        };

        self.notices.push(notice);
    }

    /// Link events become connecting/losing/regaining notices, worded by
    /// the status each change replaces.
    fn on_link(&mut self, event: LinkEvent) {
        let was_connected = event.previous.is_connected();
        let notice = match &event.status {
            LinkStatus::Connected { .. } if !was_connected => {
                Some(Notice::success(format!("Link {} connected", event.name)))
            }
            LinkStatus::Reconnecting { error, .. } if was_connected => Some(
                Notice::warning(format!("Link {} lost; reconnecting", event.name)).detail(error),
            ),
            LinkStatus::Failed(error) => {
                Some(Notice::error(format!("Link {} failed", event.name)).detail(error))
            }
            _ => None,
        };

        if let Some(notice) = notice {
            self.notices.push(notice);
        }
    }

    /// Keep the map near the selected vehicle, when following.
    fn follow_selected(&mut self) {
        if let Some(position) = self
            .selected
            .and_then(|system| self.fleet.vehicle(system))
            .and_then(|vehicle| vehicle.position.current.as_ref())
        {
            self.map.follow_to(position.pos);
        }
    }

    /// Apply the effects features and intent handlers requested.
    fn apply_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Confirm(confirmation) => {
                    self.confirmation = Some(confirmation);
                }
                Effect::SelectVehicle(system) => {
                    if self.fleet.contains(system) {
                        self.selected = Some(system);
                        self.follow_selected();
                    }
                }
                Effect::CenterMap(position) => {
                    self.map.center_on(position);
                }
                Effect::FollowVehicle(follow) => {
                    self.map.set_following(follow);
                    self.follow_selected();
                }
                Effect::RememberDirectory(path) => {
                    self.remember_directory(&path);
                }
                Effect::SaveLinks(links) => {
                    self.configuration.links = links;
                    self.save_configuration();
                }
                Effect::RecordTelemetry(record) => {
                    if record && self.recorder.is_none() {
                        self.start_recording();
                    } else if !record && let Some(recorder) = self.recorder.take() {
                        recorder.stop();
                    }
                }
                Effect::SaveSettings => {
                    self.save_configuration();
                }
            }
        }
    }

    fn remember_directory(&mut self, path: &Path) {
        let Some(directory) = path.is_dir().then_some(path).or(path.parent()) else {
            return;
        };

        if self.configuration.file_picker_path.as_deref() != Some(directory) {
            self.configuration.file_picker_path = Some(directory.to_path_buf());
            self.save_configuration();
        }
    }

    /// Perform an already-confirmed action, dispatched from the dialog.
    fn run_action(&mut self, action: Action) {
        let fleet = self.fleet.clone();
        let directory = self.configuration.file_picker_path.as_deref();
        let mut cx = Context::new(
            &fleet,
            &self.services,
            &mut self.notices,
            directory,
            &self.configuration.settings,
        );

        match action {
            Action::Arm { system, arm } => self.vehicles.perform_arm(&cx, system, arm),
            Action::ModeKind { system, kind } => {
                self.vehicles.perform_mode_kind(&cx, system, kind);
            }
            Action::Mission { plan, op, item } => {
                self.mission.perform(plan, op, item, true, &mut cx);
            }
        }

        let effects = cx.into_effects();
        self.apply_effects(effects);
    }
}

/// What the dock's tabs borrow each frame: a split borrow of the
/// application, so features never see each other's state.
struct PanelViewer<'a> {
    fleet: &'a FleetSnapshot,
    services: &'a FleetHandle,
    selected: Option<SystemId>,
    vehicles: &'a mut VehiclePanel,
    mission: &'a mut MissionEditor,
    parameters: &'a mut ParameterPanel,
    calibration: &'a mut CalibrationPanel,
    link_panel: &'a mut LinkPanel,
    log_panel: &'a mut LogPanel,
    manager: &'a mut groundhog_logic::services::LinkManager,
    map: &'a mut Map,
    configuration: &'a mut Configuration,
    notices: &'a mut Notifications,
    recorder_path: Option<&'a Path>,
    effects: &'a mut Vec<Effect>,
    mission_intents: &'a mut Vec<MissionIntent>,
    map_intents: &'a mut Vec<MapIntent>,
}

impl TabViewer for PanelViewer<'_> {
    type Tab = Panel;

    fn id(&mut self, tab: &mut Panel) -> egui::Id {
        egui::Id::new(format!("{tab:?}"))
    }

    fn title(&mut self, tab: &mut Panel) -> egui::WidgetText {
        tab.title().into()
    }

    fn is_closeable(&self, tab: &Panel) -> bool {
        tab.closable()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Panel) {
        // One split borrow: the context's pieces on one side, each panel's
        // state on the other, so features never see each other.
        let PanelViewer {
            fleet,
            services,
            selected,
            vehicles,
            mission,
            parameters,
            calibration,
            link_panel,
            log_panel,
            manager,
            map,
            configuration,
            notices,
            recorder_path,
            effects,
            mission_intents,
            map_intents,
        } = self;

        // Settings are Copy, so a context can use a snapshot while the
        // settings panel edits the real one.
        let settings = configuration.settings;

        let from_cx = match tab {
            Panel::Map => {
                view_map_tab(ui, map, fleet, *selected, mission, map_intents);
                None
            }
            Panel::Vehicles => {
                let mut cx = Context::new(
                    fleet,
                    services,
                    notices,
                    configuration.file_picker_path.as_deref(),
                    &settings,
                );
                let following = map.is_following();
                vehicles.view(ui, fleet, *selected, following, &mut cx);
                Some(cx.into_effects())
            }
            Panel::Mission => {
                mission.view(ui, fleet, *selected, mission_intents);
                None
            }
            Panel::Parameters => {
                let mut cx = Context::new(
                    fleet,
                    services,
                    notices,
                    configuration.file_picker_path.as_deref(),
                    &settings,
                );
                parameters.view(ui, fleet, *selected, &mut cx);
                Some(cx.into_effects())
            }
            Panel::Calibration => {
                let mut cx = Context::new(
                    fleet,
                    services,
                    notices,
                    configuration.file_picker_path.as_deref(),
                    &settings,
                );
                calibration.view(ui, fleet, *selected, &mut cx);
                Some(cx.into_effects())
            }
            Panel::Links => {
                let mut cx = Context::new(
                    fleet,
                    services,
                    notices,
                    configuration.file_picker_path.as_deref(),
                    &settings,
                );
                link_panel.view_editor(ui, manager, &mut cx);
                ui.add_space(6.0);
                link_panel.view_status(ui, manager, &mut cx);
                Some(cx.into_effects())
            }
            Panel::Logs => {
                log_panel.view(ui, fleet, *selected, notices);
                None
            }
            Panel::Settings => {
                view_settings_tab(ui, configuration, *recorder_path, effects);
                None
            }
        };

        if let Some(pending) = from_cx {
            effects.extend(pending);
        }
    }
}

/// The map tab: the engine plus the mission overlays when a plan shows.
#[allow(clippy::too_many_arguments)]
fn view_map_tab(
    ui: &mut egui::Ui,
    map: &mut Map,
    fleet: &FleetSnapshot,
    selected: Option<SystemId>,
    mission: &MissionEditor,
    map_intents: &mut Vec<MapIntent>,
) {
    let plan_key: PlanKey = selected.filter(|system| fleet.contains(*system));

    let plan = mission.plan(plan_key);
    let selected_item = mission.selected_item(plan_key);
    let status = mission.status(plan_key, fleet);
    let current = selected
        .and_then(|system| fleet.vehicle(system))
        .filter(|_| status == SyncStatus::Synced)
        .and_then(|vehicle| vehicle.current_mission_index());
    let transferring = plan_key.is_some_and(|system| mission.transferring(system));
    let placing = mission.is_placing(plan_key);

    let intents = map.show(
        ui,
        fleet,
        selected,
        plan,
        selected_item,
        current,
        !transferring,
        placing,
    );

    map_intents.extend(intents);
}

/// The settings tab mutates the configuration in place, and asks the
/// application to persist or to start/stop recording.
fn view_settings_tab(
    ui: &mut egui::Ui,
    configuration: &mut Configuration,
    recorder_path: Option<&Path>,
    effects: &mut Vec<Effect>,
) {
    let changes =
        crate::features::settings_panel::view(ui, &mut configuration.settings, recorder_path);

    if changes.record_telemetry {
        effects.push(Effect::RecordTelemetry(
            configuration.settings.record_telemetry,
        ));
    }
    if changes.any {
        effects.push(Effect::SaveSettings);
    }
}

impl eframe::App for Application {
    /// One frame: drain, draw, apply. Mirrors iced's update-then-view, with
    /// drawing first because egui is immediate mode.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();

        // A settings change applies on the next frame.
        let settings = self.configuration.settings;
        if self.applied_visuals != (settings.dark, settings.scale_factor) {
            self.apply_visuals();
        }

        // Expiry of toasts and link countdowns keep moving while idle.
        if self.notices.has_toasts() {
            self.ctx
                .request_repaint_after(std::time::Duration::from_millis(250));
        }

        let mut effects = Vec::new();
        let mut mission_intents = Vec::new();
        let mut map_intents = Vec::new();

        // The rail of closed-panel toggles, along the left edge.
        self.view_rail(ui);

        // The dock borrows nearly everything once, as one split borrow.
        {
            let fleet = self.fleet.clone();
            let Application {
                backend,
                services,
                dock,
                map,
                notices,
                selected,
                configuration,
                recorder,
                vehicles,
                mission,
                parameters,
                calibration,
                link_panel,
                log_panel,
                ..
            } = self;

            let manager = backend.links_mut();
            let selected = *selected;
            let recorder_path = recorder.as_ref().map(TlogRecorder::path);

            let mut viewer = PanelViewer {
                fleet: &fleet,
                services,
                selected,
                vehicles,
                mission,
                parameters,
                calibration,
                link_panel,
                log_panel,
                manager,
                map,
                configuration,
                notices,
                recorder_path,
                effects: &mut effects,
                mission_intents: &mut mission_intents,
                map_intents: &mut map_intents,
            };

            DockArea::new(dock)
                .draggable_tabs(true)
                .show_close_buttons(true)
                .show_inside(ui, &mut viewer);
        }

        // Map interactions become effects or mission intents; the mission
        // overlay draws from the editor, so its intents go back to the
        // editor.
        for intent in std::mem::take(&mut map_intents) {
            match intent {
                MapIntent::SelectVehicle(system) => {
                    effects.push(Effect::SelectVehicle(system));
                }
                MapIntent::LocateVehicle => {
                    let position = self
                        .selected
                        .and_then(|system| self.fleet.vehicle(system))
                        .and_then(|vehicle| vehicle.position.current.as_ref());
                    if let Some(position) = position {
                        effects.push(Effect::CenterMap(position.pos));
                    }
                }
                MapIntent::MarkerClick { item } => mission_intents.push(MissionIntent::Select {
                    plan: self.mission_plan_key(),
                    item: Some(item),
                }),
                MapIntent::MarkerAt { item, at } => {
                    mission_intents.push(MissionIntent::At {
                        plan: self.mission_plan_key(),
                        item,
                        at,
                    });
                }
                MapIntent::PlaceAt(at) => mission_intents.push(MissionIntent::PlaceAt {
                    plan: self.mission_plan_key(),
                    at,
                }),
            }
        }

        let mut cx = Context::new(
            &self.fleet,
            &self.services,
            &mut self.notices,
            self.configuration.file_picker_path.as_deref(),
            &self.configuration.settings,
        );
        self.mission.apply(&mission_intents, &mut cx);
        effects.extend(cx.into_effects());
        self.apply_effects(effects);

        self.draw_confirmation();
        self.draw_toasts();
    }
}

impl Application {
    /// The rail of closed-panel toggles, along the left edge.
    fn view_rail(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("groundhog.rail")
            .resizable(false)
            .show(ui, |rail| {
                for panel in Panel::DOCKABLE {
                    let open = self.dock.find_tab(&panel).is_some();
                    if !open
                        && rail
                            .add_enabled(
                                true,
                                egui::Button::new(panel.title()).fill(egui::Color32::TRANSPARENT),
                            )
                            .clicked()
                    {
                        self.dock[SurfaceIndex::main()].push_to_focused_leaf(panel);
                    }
                }
            });
    }

    /// The plan key the map and the mission panel show.
    fn mission_plan_key(&self) -> PlanKey {
        self.selected.filter(|system| self.fleet.contains(*system))
    }

    /// The pending confirmation, shown as a modal over everything.
    fn draw_confirmation(&mut self) {
        let Some(confirmation) = self.confirmation.clone() else {
            return;
        };

        let danger_fill = style::severity::CRITICAL.gamma_multiply(0.8);
        let accept_fill = if confirmation.danger {
            danger_fill
        } else {
            style::map::SELECTED.gamma_multiply(0.6)
        };

        let modal =
            egui::Modal::new(egui::Id::new("groundhog.confirmation")).show(&self.ctx, |body| {
                body.heading(&confirmation.title);
                body.add_space(6.0);
                body.label(&confirmation.body);
                body.add_space(10.0);

                body.with_layout(egui::Layout::right_to_left(egui::Align::Center), |row| {
                    row.add_space(4.0);
                    let accept = row
                        .add(
                            egui::Button::new(
                                egui::RichText::new(confirmation.accept_label).strong(),
                            )
                            .fill(accept_fill),
                        )
                        .clicked();
                    let cancel = row.button("Cancel").clicked();

                    if accept {
                        Some(Answer::Accept)
                    } else if cancel {
                        Some(Answer::Dismiss)
                    } else {
                        None
                    }
                })
                .inner
            });

        // The content closure returns `Option<Answer>`, so the modal's inner
        // is the answer when a button was clicked.
        match modal.inner {
            Some(Answer::Accept) => {
                self.confirmation = None;
                let action = confirmation.action;
                self.run_action(action);
            }
            Some(Answer::Dismiss) => {
                self.confirmation = None;
            }
            None => {}
        }
    }

    /// The toast stack, newest at the bottom, in the map area's corner.
    fn draw_toasts(&mut self) {
        let toasts: Vec<(u64, Notice)> = self
            .notices
            .toasts_with_ids()
            .map(|(id, notice)| (id, notice.clone()))
            .collect();
        if toasts.is_empty() {
            return;
        }

        egui::Area::new(egui::Id::new("groundhog.toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .order(egui::Order::Foreground)
            .show(&self.ctx, |area| {
                for (id, notice) in &toasts {
                    let (fill, text) = match notice.level {
                        Level::Error => (
                            style::notice::ERROR,
                            egui::Color32::from_rgb(0xff, 0xaa, 0xaa),
                        ),
                        Level::Warning => (
                            style::notice::WARNING,
                            egui::Color32::from_rgb(0xff, 0xe8, 0xaa),
                        ),
                        Level::Success => (style::notice::SUCCESS, style::map::LABEL),
                        Level::Info => (style::notice::INFO, style::map::LABEL),
                    };

                    egui::Frame::new()
                        .fill(fill)
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(area, |toast| {
                            toast.horizontal(|message| {
                                message.vertical(|texts| {
                                    texts.label(
                                        egui::RichText::new(&notice.title).color(text).size(13.0),
                                    );
                                    if let Some(detail) = &notice.detail {
                                        texts.label(
                                            egui::RichText::new(detail)
                                                .color(text)
                                                .size(11.0)
                                                .weak(),
                                        );
                                    }
                                });
                                if message
                                    .with_layout(
                                        egui::Layout::right_to_left(egui::Align::Min),
                                        |close| close.button("×"),
                                    )
                                    .inner
                                    .clicked()
                                {
                                    self.notices.dismiss(*id);
                                }
                            });
                        });
                    area.add_space(6.0);
                }
            });
    }
}
