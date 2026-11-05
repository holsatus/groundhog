//! The 3D globe in the map pane's place, on the desktop.
//!
//! The globe runs only while it is shown: switching to it starts one looking
//! straight down at where the map looks, at the map's scale, and switching
//! back drops it, which stops its rendering and its tile downloads. The map
//! then takes over where the globe looked, at the globe's scale there, so
//! the view goes back and forth.
//!
//! Both speak of scale as the ground the pane's height spans: the map knows
//! its meters per pixel, the globe its view, and the pane's height joins
//! them.

use groundhog_globe::{Globe, Options, Overhead, Placement};
use groundhog_logic::domain::geo::GeoPosition;
use iced::{Alignment, Element, Length, Widget, widget::container};

use super::{Application, Message};
use crate::{
    panels::notifications::Notice,
    platform,
    ui::{self, Icon},
    widget::{globe::globe, surface::surface},
};

/// The map pane's height, in logical pixels, until it is measured.
const UNMEASURED_HEIGHT_PX: f32 = 800.0;

impl Application {
    /// Show the globe in the map's place, or the map again, each where the
    /// other looked.
    pub(super) fn toggle_globe(&mut self) {
        let height_px = f64::from(
            self.map_size
                .map_or(UNMEASURED_HEIGHT_PX, |size| size.height)
                .max(1.0),
        );
        if let Some(stopped) = self.globe.take() {
            if let Some(focus) = stopped.focus() {
                let centre = GeoPosition::new(focus.lat, focus.lon);
                self.map.look_at(centre, focus.span_m / height_px);
            }
            return;
        }

        let (centre, metres_per_px) = self.map.centre();
        let options = Options {
            cache_dir: platform::cache_dir().map(|directory| directory.join("globe")),
            start: Placement::Overhead(Overhead {
                lat: centre.lat,
                lon: centre.lon,
                span_m: metres_per_px * height_px,
            }),
            ..Options::default()
        };
        match Globe::start(options) {
            Ok(started) => self.globe = Some(started),
            Err(err) => self
                .notices
                .push(Notice::error("The 3D globe could not start").detail(err.to_string())),
        }
    }

    /// The map pane's content: the globe while it is on, else the map.
    pub(super) fn view_map_or_globe(&self) -> Element<'_, Message> {
        match &self.globe {
            Some(started) => ui::picture(globe(started)).boxed(),
            None => self.view_map(super::Placement::Pane).boxed(),
        }
    }

    /// The switch between the map and the globe, in the pane's corner.
    pub(super) fn view_globe_switch(&self) -> impl Widget<Message> + '_ {
        let (icon, hint) = match self.globe {
            Some(_) => (Icon::Map, "Show the map"),
            None => (Icon::Globe, "Show the 3D globe"),
        };
        let switch = ui::icon_button(icon)
            .hint(hint)
            .on_press(Message::ToggleGlobe)
            .boxed();
        container(surface(ui::floating(switch)))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::Start)
            .padding(ui::space::S)
    }
}
