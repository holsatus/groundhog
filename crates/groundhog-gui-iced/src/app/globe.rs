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
//!
//! The globe can also ride along with the selected vehicle, through a
//! camera looking straight ahead from it. Each snapshot hands the globe the
//! vehicle's newest fix and attitude, with the vehicle's own time of each;
//! the globe smooths them into steady motion (`groundhog_globe`'s ride).
//! The vehicle is asked to send its attitude faster while it is ridden
//! with, which an autopilot's default rate is too slow for: the camera turns
//! with it, and a turn shows every step between two attitudes. It is asked
//! back to its default rate when the ride ends.

use std::time::{Duration, Instant};

use groundhog_globe::{Fix, Globe, Options, Orientation, Overhead, Placement};
use groundhog_logic::{
    SystemId, domain::geo::GeoPosition, domain::vehicle::Vehicle, services::FleetHandle,
};
use iced::{
    Alignment, Element, Length, Widget,
    widget::{column, container},
};

use super::{Application, Message};
use crate::{
    panels::notifications::Notice,
    platform,
    ui::{self, Icon},
    widget::{globe::globe, surface::surface},
};

/// The map pane's height, in logical pixels, until it is measured.
const UNMEASURED_HEIGHT_PX: f32 = 800.0;

/// How often a vehicle ridden along with is asked to send its attitude.
const RIDE_ATTITUDE_INTERVAL: Duration = Duration::from_millis(50);

/// The globe on screen, and what it rides with.
pub(super) struct Shown {
    globe: Globe,
    riding: Option<Riding>,
}

/// A ride along with a vehicle: which, and the samples of it already sent.
struct Riding {
    vehicle: SystemId,
    /// When the newest fix sent arrived.
    fix: Option<Instant>,
    /// When the newest attitude sent arrived.
    attitude: Option<Instant>,
}

impl Riding {
    fn new(vehicle: SystemId) -> Self {
        Self {
            vehicle,
            fix: None,
            attitude: None,
        }
    }

    /// The samples of `vehicle` not sent yet.
    fn news(&mut self, vehicle: &Vehicle) -> (Option<Fix>, Option<Orientation>) {
        let fix = fix_of(vehicle).filter(|fix| self.fix != Some(fix.received));
        let orientation = orientation_of(vehicle)
            .filter(|orientation| self.attitude != Some(orientation.received));
        if let Some(fix) = &fix {
            self.fix = Some(fix.received);
        }
        if let Some(orientation) = &orientation {
            self.attitude = Some(orientation.received);
        }
        (fix, orientation)
    }
}

/// The vehicle's newest fix, as the globe takes it.
fn fix_of(vehicle: &Vehicle) -> Option<Fix> {
    let position = vehicle.position.current.as_ref()?;
    Some(Fix {
        taken_s: position.time_boot_ms.map(seconds),
        received: position.at,
        lat: position.pos.lat,
        lon: position.pos.lon,
        alt_msl_m: f64::from(position.alt_msl),
        velocity_ned: vehicle
            .position
            .velocity_ned
            .map(|velocity| [velocity.x, velocity.y, velocity.z].map(f64::from)),
    })
}

/// The vehicle's newest attitude, as the globe takes it.
fn orientation_of(vehicle: &Vehicle) -> Option<Orientation> {
    let attitude = vehicle.attitude.current.as_ref()?;
    let (roll, pitch, yaw) = attitude.euler_angles();
    Some(Orientation {
        taken_s: Some(seconds(attitude.time_boot_ms)),
        received: attitude.at,
        roll: f64::from(roll),
        pitch: f64::from(pitch),
        yaw: f64::from(yaw),
    })
}

/// The vehicle asked to send its attitude faster, and the requests to the
/// vehicles, sent one after another so that a ride stopped right after it
/// started leaves the vehicle at its default rate.
#[derive(Default)]
pub(super) struct AttitudeRate {
    faster: Option<SystemId>,
    requests: Option<tokio::task::JoinHandle<()>>,
}

impl AttitudeRate {
    /// Ask `ridden` to send its attitude faster, and the vehicle asked
    /// before, if another, to send it at its default rate again. Best
    /// effort: a vehicle that refuses is ridden with as fast as it sends.
    fn follow(&mut self, fleet: &FleetHandle, ridden: Option<SystemId>) {
        let changes = self.changes(ridden);
        if changes.is_empty() {
            return;
        }
        let requests: Vec<_> = changes
            .into_iter()
            .map(|(system, interval)| (system, fleet.vehicle(system), interval))
            .collect();
        let previous = self.requests.take();
        self.requests = Some(tokio::spawn(async move {
            if let Some(previous) = previous {
                drop(previous.await);
            }
            for (system, client, interval) in requests {
                if let Err(error) = client.set_attitude_interval(interval).await {
                    log::info!("{system:?} did not change its attitude rate: {error}");
                }
            }
        }));
    }

    /// The rates to ask for, to follow `ridden`: `None` for the default.
    fn changes(&mut self, ridden: Option<SystemId>) -> Vec<(SystemId, Option<Duration>)> {
        if self.faster == ridden {
            return Vec::new();
        }
        let back = self.faster.map(|system| (system, None));
        let faster = ridden.map(|system| (system, Some(RIDE_ATTITUDE_INTERVAL)));
        self.faster = ridden;
        back.into_iter().chain(faster).collect()
    }

    /// `system` rebooted, which put its rates back to the defaults: ask
    /// again if it is still ridden with.
    pub(super) fn rebooted(&mut self, system: SystemId) {
        if self.faster == Some(system) {
            self.faster = None;
        }
    }
}

/// Milliseconds since boot, in seconds.
fn seconds(time_boot_ms: u32) -> f64 {
    f64::from(time_boot_ms) / 1000.0
}

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
            if let Some(focus) = stopped.globe.focus() {
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
            Ok(started) => {
                self.globe = Some(Shown {
                    globe: started,
                    riding: None,
                });
            }
            Err(err) => self
                .notices
                .push(Notice::error("The 3D globe could not start").detail(err.to_string())),
        }
    }

    /// Have the vehicle ridden with, if any, send its attitude faster, and
    /// no other.
    pub(super) fn sync_attitude_rate(&mut self) {
        let ridden = self
            .globe
            .as_ref()
            .and_then(|shown| shown.riding.as_ref())
            .map(|riding| riding.vehicle);
        self.attitude_rate.follow(self.backend.fleet(), ridden);
    }

    /// Ride along with the selected vehicle, or stop.
    pub(super) fn toggle_ride(&mut self) {
        let Some(shown) = &mut self.globe else {
            return;
        };
        if shown.riding.take().is_some() {
            shown.globe.controls().release();
            return;
        }
        if let Some(vehicle) = self.selected {
            shown.globe.controls().ride();
            shown.riding = Some(Riding::new(vehicle));
            self.feed_globe();
        }
    }

    /// Hand the globe what is new of the vehicle it rides with. Selecting
    /// another vehicle rides with that one instead.
    pub(super) fn feed_globe(&mut self) {
        let Some(shown) = &mut self.globe else {
            return;
        };
        let Some(riding) = &mut shown.riding else {
            return;
        };
        let controls = shown.globe.controls();
        match self.selected {
            Some(selected) if selected != riding.vehicle => {
                controls.ride();
                *riding = Riding::new(selected);
            }
            Some(_) => {}
            None => {
                controls.release();
                shown.riding = None;
                return;
            }
        }
        let Some(vehicle) = self.fleet.vehicle(riding.vehicle) else {
            return;
        };
        let (fix, orientation) = riding.news(vehicle);
        if let Some(fix) = fix {
            controls.fix(fix);
        }
        if let Some(orientation) = orientation {
            controls.orientation(orientation);
        }
    }

    /// The map pane's content: the globe while it is on, else the map.
    pub(super) fn view_map_or_globe(&self) -> Element<'_, Message> {
        match &self.globe {
            Some(shown) => ui::picture(globe(&shown.globe)).boxed(),
            None => self.view_map(super::Placement::Pane).boxed(),
        }
    }

    /// The switch between the map and the globe in the pane's top-left corner, and
    /// beneath it, while the globe shows, the ride along.
    pub(super) fn view_globe_switch(&self) -> impl Widget<Message> + '_ {
        let (icon, hint) = match self.globe {
            Some(_) => (Icon::Map, "Show the map"),
            None => (Icon::Globe, "Show the 3D globe"),
        };
        let switch = ui::icon_button(icon)
            .hint(hint)
            .on_press(Message::ToggleGlobe)
            .boxed();

        // Only over the globe: an absent child takes no room, nor the spacing before it
        let ride = self.globe.as_ref().map(|shown| {
            let riding = shown.riding.is_some();
            let hint = match (riding, self.selected) {
                (true, _) => "Stop riding along".to_owned(),
                (false, Some(vehicle)) => {
                    format!("Ride along with {}", self.fleet.vehicle_name(vehicle))
                }
                (false, None) => "Select a vehicle to ride along with".to_owned(),
            };
            let can = riding || self.selected.is_some();
            ui::icon_button(Icon::Vehicle)
                .chosen(riding)
                .hint(hint)
                .on_press_maybe(can.then_some(Message::ToggleRide))
                .boxed()
        });

        // A chip, as the video's controls are: the card around its buttons, not behind them
        container(surface(
            ui::floating(column![switch, ride].spacing(ui::space::S)).padding(ui::space::S),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        // Clear of the heading indicator in the opposite corner
        .align_x(Alignment::Start)
        .align_y(Alignment::Start)
        .padding(ui::space::S)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use groundhog_logic::{MavlinkId, domain::fleet::Fleet};
    use mavio::{
        DefaultDialect,
        default_dialect::{
            enums::{MavAutopilot, MavModeFlag, MavState, MavType},
            messages,
        },
    };

    use super::*;

    const AUTOPILOT: MavlinkId = MavlinkId {
        system: 1,
        component: 1,
    };

    fn fleet() -> (Fleet, Instant) {
        let mut fleet = Fleet::default();
        let at = Instant::now();
        fleet.observe(
            AUTOPILOT,
            at,
            Some(&DefaultDialect::Heartbeat(messages::Heartbeat {
                type_: MavType::FixedWing,
                autopilot: MavAutopilot::Ardupilotmega,
                base_mode: MavModeFlag::empty(),
                system_status: MavState::Active,
                ..Default::default()
            })),
        );
        (fleet, at)
    }

    fn report(fleet: &mut Fleet, message: DefaultDialect, at: Instant) {
        fleet.observe(AUTOPILOT, at, Some(&message));
    }

    #[test]
    fn a_ride_is_handed_each_sample_once_with_the_vehicles_own_time() {
        let (mut fleet, start) = fleet();
        let at = start + Duration::from_millis(100);
        report(
            &mut fleet,
            DefaultDialect::GlobalPositionInt(messages::GlobalPositionInt {
                time_boot_ms: 61_500,
                lat: 556_761_000,
                lon: 125_683_000,
                alt: 120_000,
                vx: 1500,
                ..Default::default()
            }),
            at,
        );
        report(
            &mut fleet,
            DefaultDialect::Attitude(messages::Attitude {
                time_boot_ms: 61_520,
                yaw: 1.0,
                ..Default::default()
            }),
            at,
        );
        let snapshot = fleet.snapshot();
        let vehicle = snapshot.vehicle(SystemId(1)).expect("the vehicle");

        let mut riding = Riding::new(SystemId(1));
        let (fix, orientation) = riding.news(vehicle);
        let fix = fix.expect("a fix");
        assert_eq!(fix.taken_s, Some(61.5));
        assert_eq!(fix.alt_msl_m, 120.0);
        assert!((fix.lat - 55.6761).abs() < 1e-9);
        assert_eq!(fix.velocity_ned, Some([15.0, 0.0, 0.0]));
        let orientation = orientation.expect("an orientation");
        assert_eq!(orientation.taken_s, Some(61.52));
        assert!((orientation.yaw - 1.0).abs() < 1e-6);

        // The next snapshot holds the same samples: nothing new to hand on
        assert_eq!(riding.news(vehicle), (None, None));
    }

    #[test]
    fn a_ridden_vehicle_sends_its_attitude_faster_until_the_ride_ends() {
        let (a, b) = (SystemId(1), SystemId(2));
        let faster = Some(RIDE_ATTITUDE_INTERVAL);
        let mut rate = AttitudeRate::default();

        assert_eq!(rate.changes(Some(a)), [(a, faster)]);
        // Each snapshot asks again what is ridden: nothing changed
        assert_eq!(rate.changes(Some(a)), []);
        // Riding with another vehicle puts the first back at its default
        assert_eq!(rate.changes(Some(b)), [(a, None), (b, faster)]);

        // A reboot forgets the rate asked for, so it is asked again
        rate.rebooted(a);
        assert_eq!(rate.changes(Some(b)), []);
        rate.rebooted(b);
        assert_eq!(rate.changes(Some(b)), [(b, faster)]);

        // The ride ends: the default again
        assert_eq!(rate.changes(None), [(b, None)]);
        assert_eq!(rate.changes(None), []);
    }
}
