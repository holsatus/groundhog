//! The panels the application shows, listed once.
//!
//! [`panels!`] turns the list at the bottom into everything the application
//! needs to hold and route to its panels: [`PanelMessage`] (a variant per
//! panel), [`Panels`] (their state), and the methods that dispatch a
//! message, build a view, collect map layers and pass a fleet snapshot on.
//! Adding a panel is a line here, beside its [`layout::Panel`] variant and
//! its place in a [`shell::Destination`](crate::shell::Destination).

use std::{path::Path, time::Instant};

use groundhog_logic::{
    domain::fleet::FleetSnapshot,
    services::{FleetHandle, LinkManager},
};
use iced::{Element, Task, Widget};

use super::Message;
use crate::{
    config::Settings,
    features::{
        Context, Effect, MapContext, Panel, ViewContext, calibration::CalibrationPanel,
        link_panel::LinkPanel, log_panel::LogPanel, mission::MissionEditor,
        notifications::Notifications, parameter_panel::ParameterPanel,
        settings_panel::SettingsPanel, vehicles::VehiclePanel, video::VideoPanel,
    },
    layout,
    map::LayerSet,
};

/// What a panel's update is given, borrowed from the application, and the
/// effects its panels request.
pub(super) struct PanelEnv<'a> {
    pub(super) fleet: &'a FleetSnapshot,
    pub(super) services: &'a FleetHandle,
    pub(super) links: &'a LinkManager,
    pub(super) notices: &'a mut Notifications,
    pub(super) directory: Option<&'a Path>,
    pub(super) settings: &'a Settings,
    pub(super) effects: Vec<Effect<PanelMessage>>,
}

impl PanelEnv<'_> {
    /// Run `run` on `panel` with a [`Context`] of its own, keeping the
    /// effects it requests, in the panels' message.
    fn run<P: Panel>(
        &mut self,
        panel: &mut P,
        wrap: fn(P::Message) -> PanelMessage,
        run: impl FnOnce(&mut P, &mut Context<'_, P::Message>) -> Task<P::Message>,
    ) -> Task<PanelMessage> {
        let mut cx = Context::new(
            self.fleet,
            self.services,
            self.links,
            &mut *self.notices,
            self.directory,
            self.settings,
        );
        let task = run(panel, &mut cx);
        self.effects
            .extend(cx.into_effects().into_iter().map(|effect| effect.map(wrap)));
        task.map(wrap)
    }
}

macro_rules! panels {
    ($($variant:ident: $field:ident: $panel:ty),* $(,)?) => {
        /// A message to one of the panels.
        #[derive(Debug, Clone)]
        pub enum PanelMessage {
            $($variant(<$panel as Panel>::Message),)*
        }

        $(
            impl From<<$panel as Panel>::Message> for PanelMessage {
                fn from(message: <$panel as Panel>::Message) -> Self {
                    PanelMessage::$variant(message)
                }
            }

            impl From<<$panel as Panel>::Message> for Message {
                fn from(message: <$panel as Panel>::Message) -> Self {
                    Message::Panel(PanelMessage::$variant(message))
                }
            }
        )*

        /// Every panel's state.
        pub struct Panels {
            $(pub(super) $field: $panel,)*
        }

        impl Panels {
            /// The workspace's panels these are, in the list's order.
            #[cfg(test)]
            pub(super) const LISTED: &[layout::Panel] = &[$(layout::Panel::$variant,)*];

            /// Route `message` to its panel.
            pub(super) fn update(
                &mut self,
                message: PanelMessage,
                env: &mut PanelEnv<'_>,
            ) -> Task<PanelMessage> {
                match message {
                    $(PanelMessage::$variant(message) => env.run(
                        &mut self.$field,
                        PanelMessage::$variant,
                        |panel, cx| panel.update(message, cx),
                    ),)*
                }
            }

            /// Tell every panel the fleet published (`env.fleet`).
            pub(super) fn on_fleet(&mut self, env: &mut PanelEnv<'_>) -> Task<PanelMessage> {
                Task::batch([
                    $(env.run(&mut self.$field, PanelMessage::$variant, |panel, cx| {
                        panel.on_fleet(cx)
                    }),)*
                ])
            }

            /// Whether any panel must hear the time pass.
            pub(super) fn wants_ticks(&self) -> bool {
                false $(|| self.$field.wants_ticks())*
            }

            /// Bring every panel up to date as of `now`.
            pub(super) fn on_tick(
                &mut self,
                now: Instant,
                env: &mut PanelEnv<'_>,
            ) -> Task<PanelMessage> {
                Task::batch([
                    $(env.run(&mut self.$field, PanelMessage::$variant, |panel, cx| {
                        panel.on_tick(now, cx)
                    }),)*
                ])
            }

            /// The view of `panel`, if it is one of these.
            pub(super) fn view<'a>(
                &'a self,
                panel: layout::Panel,
                cx: ViewContext<'a>,
            ) -> Option<Element<'a, PanelMessage>> {
                match panel {
                    $(layout::Panel::$variant => {
                        Some(self.$field.view(cx).map(PanelMessage::$variant).boxed())
                    })*
                    _ => None,
                }
            }

            /// Every panel's map layers.
            pub(super) fn layers<'a>(
                &'a self,
                map: MapContext<'a>,
                layers: &mut LayerSet<'a, PanelMessage>,
            ) {
                $({
                    let mut own = LayerSet::new();
                    self.$field.layers(map, &mut own);
                    layers.extend(own, PanelMessage::$variant);
                })*
            }
        }
    };
}

panels! {
    Vehicles: vehicles: VehiclePanel,
    Mission: mission: MissionEditor,
    Parameters: parameters: ParameterPanel,
    Calibration: calibration: CalibrationPanel,
    Links: links: LinkPanel,
    Video: video: VideoPanel,
    Logs: logs: LogPanel,
    Settings: settings: SettingsPanel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Destination;

    /// Every panel the shells offer is listed once, apart from the map,
    /// which is the application's own.
    #[test]
    fn every_offered_panel_is_listed_once() {
        let offered: Vec<layout::Panel> = Destination::ALL
            .into_iter()
            .flat_map(Destination::panels)
            .copied()
            .filter(|panel| *panel != layout::Panel::Map)
            .collect();

        let mut listed = Panels::LISTED.to_vec();
        listed.sort_by_key(|panel| format!("{panel:?}"));
        let mut expected = offered.clone();
        expected.sort_by_key(|panel| format!("{panel:?}"));
        assert_eq!(listed, expected);
    }
}
