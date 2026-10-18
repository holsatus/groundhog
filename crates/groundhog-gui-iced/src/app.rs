//! The application root: composition, message routing, and policy.
//!
//! The root wires the layers together at boot, routes messages to features,
//! applies the [`Effect`]s features request, and decides which fleet and link
//! events become operator notices. It holds the latest fleet snapshot, which
//! the fleet runtime publishes from off the UI thread.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use groundhog_logic::{
    Backend, BackendOptions, SystemId,
    domain::{
        calibration::Phase,
        fleet::{FleetEvent, FleetSnapshot},
        vehicle::{Liveness, status},
    },
    services::{FleetUpdate, LinkEvent, LinkStatus, TlogRecorder},
};
use iced::{
    Alignment, Element, Length, Padding, Subscription, Task, Theme, Widget, keyboard,
    widget::{Space, button, column, container, mouse_area, row, rule, sensor, stack, tooltip},
};
use mavio::default_dialect::enums::MavSeverity;

use crate::{
    config::{self, ConfigWriter, Configuration},
    features::{
        Context, Effect, MapContext,
        calibration::{self, CalibrationPanel},
        confirm::{self, Answer, Confirmation},
        link_panel::{self, LinkPanel},
        log_panel::{self, LogPanel},
        mission::{self, MissionEditor},
        notifications::{self, Level, Notice, Notifications},
        parameter_panel::{self, ParameterPanel},
        settings_panel,
        vehicles::{self, VehiclePanel},
    },
    layout::{self, Layout},
    map::{self, LayerSet, Map},
    platform::{self, Insets},
    ui::{
        self, Icon,
        style::{self, ButtonKind},
    },
    widget::{
        drag_source::drag_source,
        pane_grid::{self, PaneGrid},
    },
};

/// The features that take a [`Context`], grouped so their state can be
/// borrowed apart from the context's.
#[derive(Default)]
struct Features {
    vehicles: VehiclePanel,
    mission: MissionEditor,
    parameters: ParameterPanel,
    calibration: CalibrationPanel,
}

pub struct Application {
    layout: Layout,
    map: Map,
    configuration: Configuration,
    config_writer: ConfigWriter,
    backend: Backend,
    /// The fleet as last published by the runtime.
    fleet: Arc<FleetSnapshot>,
    /// The vehicle the UI shows.
    selected: Option<SystemId>,
    /// The keyboard modifiers held, for the rail's clicks.
    modifiers: keyboard::Modifiers,
    /// The panel whose rail button is being dragged onto the grid.
    rail_drag: Option<layout::Panel>,
    recorder: Option<TlogRecorder>,
    notices: Notifications,
    confirmation: Option<Confirmation<Message>>,
    features: Features,
    link_panel: LinkPanel,
    log_panel: LogPanel,
    /// How much of the window the platform's own chrome covers.
    insets: Insets,
    /// The theme whose system-bar icons were requested, if any.
    system_bars: Option<bool>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The fleet runtime published.
    Fleet(FleetUpdate),
    /// A link's supervisor changed status.
    Link(LinkEvent),
    /// A configuration write finished.
    ConfigurationSaved(Result<(), String>),

    Layout(layout::Message),
    /// A rail button was clicked with the primary button; the modifiers
    /// held decide where its panel opens.
    Rail(layout::Panel),
    /// A rail button is being dragged; the grid shows where its panel
    /// would land.
    RailDragged(layout::Panel),
    /// The rail button's drag ended: dropped (the grid reports where, as a
    /// layout message) or cancelled.
    RailDropped,
    /// The keyboard modifiers changed.
    Modifiers(keyboard::Modifiers),
    Map(map::Message),
    Vehicles(vehicles::Message),
    Mission(mission::Message),
    Parameters(parameter_panel::Message),
    Calibration(calibration::Message),
    Links(link_panel::Message),
    Logs(log_panel::Message),
    Settings(settings_panel::Message),
    Notifications(notifications::Message),
    Confirm(Answer),

    /// Debounce tick that flushes a pending layout save.
    LayoutSaveTick,
    /// Refreshes time-dependent views (toast expiry, link countdowns).
    Tick(Instant),
    /// The platform's chrome moved, appeared or disappeared (the status and
    /// navigation bars, the soft keyboard, a rotation).
    Insets(Insets),
}

/// Idle time after the last layout change before the layout is persisted.
const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

impl Application {
    pub fn boot() -> (Self, Task<Message>) {
        let mut notices = Notifications::default();

        let configuration = Configuration::initialize().unwrap_or_else(|error| {
            notices.push(Notice::error("Could not read the configuration").detail(error));
            Configuration::default()
        });

        // The backend spawns its tasks here: boot runs inside the
        // application's async runtime.
        let (backend, events) = Backend::start(BackendOptions {
            links: configuration.links.clone(),
            ..BackendOptions::default()
        });

        let (config_writer, saved) = config::spawn_writer();

        let layout = configuration
            .workspace
            .clone()
            .map_or_else(Layout::new, Layout::from_workspace);

        let mut application = Application {
            layout,
            map: Map::new(),
            configuration,
            config_writer,
            backend,
            fleet: Arc::default(),
            selected: None,
            modifiers: keyboard::Modifiers::default(),
            rail_drag: None,
            recorder: None,
            notices,
            confirmation: None,
            features: Features::default(),
            link_panel: LinkPanel::new(),
            log_panel: LogPanel::new(),
            insets: platform::insets(),
            system_bars: None,
        };

        if application.configuration.settings.record_telemetry {
            application.start_recording();
        }

        application.sync_system_bars();

        let saved = futures::stream::unfold(saved, |mut saved| async move {
            saved.recv().await.map(|outcome| (outcome, saved))
        });

        let tasks = Task::batch([
            Task::stream(events.fleet.into_stream()).map(Message::Fleet),
            Task::stream(events.links.into_stream()).map(Message::Link),
            Task::stream(saved).map(Message::ConfigurationSaved),
        ]);

        (application, tasks)
    }

    pub fn theme(&self) -> Theme {
        self.configuration.settings.theme.theme()
    }

    pub fn title(&self) -> String {
        "Holsatus Groundhog".to_string()
    }

    pub fn scale_factor(&self) -> f32 {
        self.configuration.settings.scale_factor
    }

    fn save_configuration(&self) {
        self.config_writer.save(self.configuration.clone());
    }

    fn start_recording(&mut self) {
        let Some(directory) = config::data_dir().map(|directory| directory.join("tlogs")) else {
            self.notices
                .push(Notice::error("No data directory to record telemetry into"));
            return;
        };

        match self.backend.record(&directory) {
            Ok(recorder) => self.recorder = Some(recorder),
            Err(error) => self
                .notices
                .push(Notice::error("Could not start telemetry recording").detail(error)),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Insets(insets) => {
                self.insets = insets;
                self.sync_system_bars();
                Task::none()
            }
            Message::Fleet(update) => {
                self.on_fleet(update);
                Task::none()
            }
            Message::Link(event) => {
                self.on_link(event);
                Task::none()
            }
            Message::ConfigurationSaved(result) => {
                if let Err(error) = result {
                    self.notices
                        .push(Notice::error("Could not save the configuration").detail(error));
                }
                Task::none()
            }
            Message::Layout(message) => {
                if self.layout.update(message) {
                    self.configuration.workspace = Some(self.layout.workspace().clone());
                }
                Task::none()
            }
            Message::Rail(panel) => {
                // A plain click opens on the left; the modifiers stand in
                // for the middle and right buttons a trackpad lacks.
                let slot = if self.modifiers.command() {
                    layout::Slot::Middle
                } else if self.modifiers.shift() {
                    layout::Slot::Right
                } else {
                    layout::Slot::Left
                };
                self.update(Message::Layout(layout::Message::Toggle(panel, slot)))
            }
            Message::RailDragged(panel) => {
                self.rail_drag = Some(panel);
                Task::none()
            }
            Message::RailDropped => {
                self.rail_drag = None;
                Task::none()
            }
            Message::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::Map(message) => self.map.update(message).map(Message::Map),
            Message::Vehicles(message) => self.dispatch(Message::Vehicles, |features, cx| {
                features.vehicles.update(message, cx)
            }),
            Message::Mission(message) => self.dispatch(Message::Mission, |features, cx| {
                features.mission.update(message, cx)
            }),
            Message::Parameters(message) => self.dispatch(Message::Parameters, |features, cx| {
                features.parameters.update(message, cx)
            }),
            Message::Calibration(message) => self.dispatch(Message::Calibration, |features, cx| {
                features.calibration.update(message, cx)
            }),
            Message::Links(message) => {
                let (task, event) =
                    self.link_panel
                        .update(message, self.backend.links_mut(), &mut self.notices);

                if let Some(link_panel::Event::LinksChanged(links)) = event {
                    self.configuration.links = links;
                    self.save_configuration();
                }
                task.map(Message::Links)
            }
            Message::Logs(message) => {
                self.log_panel.update(message);
                Task::none()
            }
            Message::Settings(message) => {
                self.on_settings(message);
                Task::none()
            }
            Message::Notifications(message) => {
                self.notices.update(message);
                Task::none()
            }
            Message::Confirm(answer) => match (answer, self.confirmation.take()) {
                (Answer::Accept, Some(confirmation)) => Task::done(confirmation.accept),
                _ => Task::none(),
            },
            Message::LayoutSaveTick => {
                if self.layout.is_dirty() {
                    self.layout.clear_dirty();
                    self.save_configuration();
                }
                Task::none()
            }
            Message::Tick(now) => {
                self.notices.update(notifications::Message::Tick(now));
                Task::none()
            }
        }
    }

    /// Run a feature update with a fresh [`Context`], then apply the effects
    /// it requested.
    fn dispatch<M>(
        &mut self,
        wrap: fn(M) -> Message,
        update: impl FnOnce(&mut Features, &mut Context<M>) -> Task<M>,
    ) -> Task<Message>
    where
        M: Send + 'static,
    {
        let fleet = self.fleet.clone();
        let mut context = Context::new(
            &fleet,
            self.backend.fleet(),
            &mut self.notices,
            self.configuration.file_picker_path.as_deref(),
            &self.configuration.settings,
        );

        let task = update(&mut self.features, &mut context).map(wrap);
        let effects = context.into_effects();

        for effect in effects {
            self.apply(effect.map(wrap));
        }

        task
    }

    fn apply(&mut self, effect: Effect<Message>) {
        match effect {
            Effect::Confirm(confirmation) => self.confirmation = Some(confirmation),
            Effect::SelectVehicle(system) => {
                if self.fleet.contains(system) {
                    self.selected = Some(system);
                    self.follow_selected();
                }
            }
            Effect::CenterMap(position) => {
                self.map.set_following(false);
                self.map.center_on(position);
            }
            Effect::FollowVehicle(follow) => {
                self.map.set_following(follow);
                self.follow_selected();
            }
            Effect::RememberDirectory(path) => self.remember_directory(&path),
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

    /// Keep the map on the selected vehicle, if following.
    fn follow_selected(&mut self) {
        if let Some(position) = self
            .selected
            .and_then(|system| self.fleet.vehicle(system))
            .and_then(|vehicle| vehicle.position.current.as_ref())
        {
            self.map.follow_to(position.pos);
        }
    }

    fn on_fleet(&mut self, update: FleetUpdate) {
        self.fleet = update.snapshot;
        self.features.mission.sync(&self.fleet);

        for event in update.events {
            self.on_fleet_event(event);
        }

        self.follow_selected();
    }

    /// Fleet events become selection changes and operator notices.
    fn on_fleet_event(&mut self, event: FleetEvent) {
        let name = |system| self.fleet.vehicle_name(system);

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
                    Phase::Succeeded => Notice::success(format!("{subject} succeeded")),
                    Phase::Failed(reason) => {
                        Notice::error(format!("{subject} failed")).detail(reason)
                    }
                    Phase::Cancelled => Notice::info(format!("{subject} cancelled")),
                    Phase::Starting | Phase::Running => return,
                }
                .vehicle(system)
            }
        };

        self.notices.push(notice);
    }

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

    fn on_settings(&mut self, message: settings_panel::Message) {
        let settings = &mut self.configuration.settings;
        let before = *settings;

        match message {
            settings_panel::Message::Theme(mode) => settings.theme = mode,
            settings_panel::Message::CommitScaleFactor => {
                settings.scale_factor = settings.temp_scale_factor
            }
            settings_panel::Message::SetScaleFactor(scale) => settings.temp_scale_factor = scale,
            settings_panel::Message::DefaultAltitude(altitude) => {
                settings.default_waypoint_altitude_m = altitude;
            }
            settings_panel::Message::RecordTelemetry(record) => {
                settings.record_telemetry = record;
                if record && self.recorder.is_none() {
                    self.start_recording();
                } else if !record && let Some(recorder) = self.recorder.take() {
                    recorder.stop();
                }
            }
            settings_panel::Message::RestoreDefaults => *settings = Default::default(),
        }

        if self.configuration.settings != before {
            self.save_configuration();
        }

        self.sync_system_bars();
    }

    /// Matches the status- and navigation-bar icons to the theme: dark icons
    /// on a light palette, light ones on a dark palette.
    fn sync_system_bars(&mut self) {
        let dark = self.theme().palette().is_dark;

        if self.system_bars != Some(dark) {
            self.system_bars = Some(dark);
            platform::set_system_bars(dark);
        }
    }

    pub fn view(&self) -> impl Widget<Message> + '_ {
        // The rail and the panes float on the window's background, one gap
        // apart from each other and from the window's edges.
        let base = container(row![self.view_rail(), self.view_panes()].spacing(ui::space::S))
            .padding(ui::space::S)
            .style(style::workspace);

        // Toasts pop up in the map's corner, unless a maximized pane hides
        // the map.
        let base = if self.map_is_shown() {
            base.boxed()
        } else {
            stack![base, self.view_toasts(ui::space::XL)].boxed()
        };

        container(confirm::view(
            base,
            self.confirmation.as_ref(),
            Message::Confirm,
        ))
        .padding(self.insets.padding())
    }

    /// Whether the map is on screen: no other pane is maximized.
    fn map_is_shown(&self) -> bool {
        let panes = self.layout.panes();
        panes
            .maximized()
            .is_none_or(|pane| panes.get(pane) == Some(&layout::Panel::Map))
    }

    /// The toasts, in the bottom-right corner of the space they are given,
    /// `inset` from its edges.
    fn view_toasts(&self, inset: f32) -> Element<'_, Message> {
        container(self.notices.view().map(Message::Notifications))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(inset)
            .boxed()
    }

    /// The rail: a button per panel, highlighted while it is open. Pressing
    /// an open panel's button closes it. A closed panel opens in the left,
    /// middle or right column by the mouse button pressed (or a plain,
    /// Ctrl/Cmd or Shift click), or where its button is dragged onto the
    /// grid.
    fn view_rail(&self) -> Element<'_, Message> {
        let buttons = layout::Panel::DOCKABLE.into_iter().map(|panel| {
            let open = self.layout.is_open(panel);
            let icon = ui::icon::icon(panel.icon(), ui::size::RAIL_ICON, ButtonKind::Subtle, true);
            let button = button(container(icon).center(Length::Fill))
                .padding(0)
                .width(ui::size::RAIL_BUTTON)
                .height(ui::size::RAIL_BUTTON)
                .style(style::rail_button(open))
                .on_press(Message::Rail(panel));
            let toggle = |slot| Message::Layout(layout::Message::Toggle(panel, slot));
            let button = mouse_area(button)
                .on_middle_press(toggle(layout::Slot::Middle))
                .on_right_press(toggle(layout::Slot::Right));

            // Dragged, the button becomes a label for the panel, styled as a tooltip.
            let preview = container(
                row![
                    ui::icon::icon(panel.icon(), ui::size::ICON, ButtonKind::Subtle, true),
                    ui::text(panel.title())
                ]
                .spacing(ui::space::S)
                .align_y(Alignment::Center),
            )
            .padding([ui::space::S, ui::space::M])
            .style(style::tooltip);
            let button = drag_source(
                button,
                preview,
                Message::RailDragged(panel),
                Message::RailDropped,
            );

            ui::tooltip(button, panel.title(), tooltip::Position::Right)
        });

        column(buttons).spacing(ui::space::S).boxed()
    }

    /// The panes, as the layout arranges them. The layout is told the
    /// grid's size, which it needs to keep the side columns' widths in
    /// pixels.
    fn view_panes(&self) -> impl Widget<Message> + '_ {
        let sized = |size| Message::Layout(layout::Message::Sized(size));
        let mut grid = PaneGrid::new(self.layout.panes(), |pane, panel, is_maximized| {
            // The map paints opaque tiles across its whole pane, which a rounded
            // container cannot clip; it draws its own frame instead (see
            // `map::Map::view`), and has no title bar to grab, so it cannot be
            // moved or closed.
            if panel.is_fixed() {
                let empty = Space::new().height(Length::Fixed(0.0)).boxed();
                return pane_grid::Content::new(self.pane_body(*panel))
                    .title_bar(pane_grid::TitleBar::new(empty).padding(0));
            }

            // Every other pane is a rounded card, its header split off by a
            // rule.
            let body = column![
                rule::horizontal(style::PANE_BORDER_WIDTH).style(style::pane_divider),
                self.pane_body(*panel),
            ]
            .height(Length::Fill);

            let layout = |message| Message::Layout(message);
            let size = if is_maximized {
                ui::icon_button(Icon::Minimize)
                    .hint("Restore")
                    .on_press(layout(layout::Message::Restore))
            } else {
                ui::icon_button(Icon::Maximize)
                    .hint("Maximize")
                    .on_press(layout(layout::Message::Maximize(pane)))
            };
            let close = ui::icon_button(Icon::Close)
                .hint("Close")
                .on_press(layout(layout::Message::Close(pane)));

            let title = row![
                ui::icon::icon(panel.icon(), ui::size::ICON, ButtonKind::Subtle, true),
                ui::text(panel.title())
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center);

            let title_bar = pane_grid::TitleBar::new(title.boxed())
                .controls(pane_grid::Controls::new(
                    row![size.boxed(), close.boxed()].spacing(ui::space::XS),
                ))
                // The controls are as far from the bar's right edge as from
                // its top and bottom; the title gets more room on the left.
                // The pane's border is drawn over the bar's top and right
                // edges, while the divider lies below it, so those sides add
                // the border's width.
                .padding(Padding {
                    top: ui::space::XS + style::PANE_BORDER_WIDTH,
                    right: ui::space::XS + style::PANE_BORDER_WIDTH,
                    bottom: ui::space::XS,
                    left: ui::space::M,
                })
                .style(style::title_bar(self.layout.is_focused(pane)));

            pane_grid::Content::new(body.boxed())
                .style(style::pane)
                .title_bar(title_bar)
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(layout::SPACING)
        .min_size(layout::MIN_PANE)
        .style(style::pane_grid)
        .on_click(|pane| Message::Layout(layout::Message::Clicked(pane)))
        .on_drag(|event| Message::Layout(layout::Message::Dragged(event)))
        .drop_target(|dragged, cursor, regions| self.layout.drop_hint(dragged, cursor, regions))
        .on_resize(7, |event| Message::Layout(layout::Message::Resized(event)));

        if let Some(panel) = self.rail_drag {
            grid = grid.incoming(
                move |cursor, regions| self.layout.incoming_hint(panel, cursor, regions),
                move |target| Message::Layout(layout::Message::Placed(panel, target)),
            );
        }

        sensor(grid).on_show(sized).on_resize(sized)
    }

    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        let fleet: &FleetSnapshot = &self.fleet;

        match panel {
            layout::Panel::Map => {
                let theme = self.theme();
                let context = MapContext {
                    fleet,
                    selected: self.selected,
                    palette: *theme.palette(),
                };

                let mut layers = LayerSet::new();
                layers.plugin(&self.features.vehicles, context, Message::Vehicles);
                layers.plugin(&self.features.mission, context, Message::Mission);

                let map = self.map.view(layers, Message::Map, &theme);
                // A `Stack` hides the pointer from the map only where an
                // upper layer shows one: here, over the toasts, which are
                // meant to hide it.
                stack![map, self.view_toasts(ui::space::M)].boxed()
            }
            layout::Panel::Vehicles => self
                .features
                .vehicles
                .view(fleet, self.selected, self.map.is_following())
                .map(Message::Vehicles)
                .boxed(),
            layout::Panel::Parameters => self
                .features
                .parameters
                .view(fleet, self.selected)
                .map(Message::Parameters)
                .boxed(),
            layout::Panel::Calibration => self
                .features
                .calibration
                .view(fleet, self.selected)
                .map(Message::Calibration)
                .boxed(),
            layout::Panel::Mission => self
                .features
                .mission
                .view(fleet, self.selected)
                .map(Message::Mission)
                .boxed(),
            layout::Panel::Links => self
                .link_panel
                .view(self.backend.links())
                .map(Message::Links)
                .boxed(),
            layout::Panel::Logs => self
                .log_panel
                .view(fleet, self.selected, &self.notices)
                .map(Message::Logs)
                .boxed(),
            layout::Panel::Settings => settings_panel::view(
                &self.configuration.settings,
                self.recorder.as_ref().map(TlogRecorder::path),
            )
            .map(Message::Settings)
            .boxed(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Trailing debounce for layout persistence: the timer is restarted on
        // every layout change (the revision is part of the subscription
        // identity), so the write happens `LAYOUT_SAVE_DEBOUNCE` after the last
        // change rather than repeatedly throughout a drag/resize.
        let layout_tick = if self.layout.is_dirty() {
            iced::time::every(LAYOUT_SAVE_DEBOUNCE)
                .with(self.layout.revision())
                .map(|_| Message::LayoutSaveTick)
        } else {
            Subscription::none()
        };

        // Time-dependent views only need refreshing while they show something.
        let active_links = self
            .backend
            .links()
            .entries()
            .any(|(_, _, status)| status.is_active());
        let refresh = if self.notices.has_toasts() || active_links {
            iced::time::every(Duration::from_secs(1)).map(Message::Tick)
        } else {
            Subscription::none()
        };

        let modifiers = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::Modifiers(modifiers))
            }
            _ => None,
        });

        Subscription::batch([
            layout_tick,
            refresh,
            modifiers,
            platform::insets_subscription().map(Message::Insets),
        ])
    }
}

#[cfg(test)]
mod gallery;
