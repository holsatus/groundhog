//! The vehicle's cameras: what each can do, as its `CAMERA_INFORMATION`
//! says, and what it reports tracking (`CAMERA_TRACKING_IMAGE_STATUS`).
//!
//! A camera is a MAVLink component of its own, or one of up to six attached
//! to the autopilot without a component id of its own, which the autopilot
//! answers for ([`CameraId`]). Cameras send their information only when
//! asked: the backend asks the autopilot when a vehicle registers, and a
//! component when its heartbeat first says it is a camera.
//!
//! Positions in the picture are the camera's: normalized to `0..=1`, from
//! the top left corner of the image ([`ImagePoint`]), whatever size it is
//! shown at.
//!
//! A camera reports what it tracks only while it tracks, and may fall
//! silent when it stops; a report older than [`TRACKING_STALE`] is no
//! longer believed. Cameras keep to the rate they are asked for loosely:
//! one asked for ten reports a second may send one, late.

use std::{collections::BTreeMap, time::Duration, time::Instant};

use mavio::default_dialect::{
    enums::{CameraCapFlags, CameraTrackingMode, CameraTrackingStatusFlags},
    messages,
};

use super::string_from_cstring;

/// How long a tracking report is believed without another: a camera that
/// reports once a second, a second late, is still tracking.
pub const TRACKING_STALE: Duration = Duration::from_secs(3);

/// Which camera: a component, and which of the cameras it answers for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CameraId {
    pub component: u8,
    /// 0 when the component is the camera; 1 to 6 for a camera attached to
    /// the autopilot (`component`), which has no component id of its own.
    pub device: u8,
}

/// A position in the picture, normalized to `0..=1` from its top left.
///
/// Always within the picture: the only way to make one is [`Self::new`],
/// which refuses NaN and values outside `0..=1`. `MAV_CMD_CAMERA_TRACK_*`
/// gives NaN no meaning, and [`ImageRect::spanning`]'s `min`/`max` would
/// silently drop one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePoint {
    x: f32,
    y: f32,
}

impl ImagePoint {
    /// `(x, y)`, if both are numbers within the picture.
    pub fn new(x: f32, y: f32) -> Option<Self> {
        let within = |value: f32| (0.0..=1.0).contains(&value);
        (within(x) && within(y)).then_some(Self { x, y })
    }

    /// From the left edge, `0..=1`.
    pub fn x(self) -> f32 {
        self.x
    }

    /// From the top edge, `0..=1`.
    pub fn y(self) -> f32 {
        self.y
    }
}

/// A rectangle in the picture, its corners normalized as [`ImagePoint`]s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageRect {
    pub top_left: ImagePoint,
    pub bottom_right: ImagePoint,
}

impl ImageRect {
    /// The rectangle with `a` and `b` as opposite corners, in any order.
    pub fn spanning(a: ImagePoint, b: ImagePoint) -> Self {
        Self {
            top_left: ImagePoint {
                x: a.x.min(b.x),
                y: a.y.min(b.y),
            },
            bottom_right: ImagePoint {
                x: a.x.max(b.x),
                y: a.y.max(b.y),
            },
        }
    }
}

/// What a camera can track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrackingSupport {
    pub point: bool,
    pub rectangle: bool,
}

impl TrackingSupport {
    pub fn any(self) -> bool {
        self.point || self.rectangle
    }
}

/// What a camera reports about its tracking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tracking {
    /// Not tracking.
    Idle,
    /// Tracking; where, if the report says.
    Active(Option<Target>),
    /// Tracking failed (the target was lost).
    Error,
}

/// Where a tracked target is in the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Point {
        at: ImagePoint,
        /// As a share of the picture's width.
        radius: Option<f32>,
    },
    Rectangle(ImageRect),
}

/// One camera.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub tracking: TrackingSupport,
    /// The latest tracking report, and when it came.
    report: Option<(Instant, Tracking)>,
}

impl Camera {
    /// What the camera last reported about its tracking, unless that is
    /// older than [`TRACKING_STALE`] at `now`.
    pub fn tracking(&self, now: Instant) -> Option<Tracking> {
        self.report
            .filter(|(at, _)| now.saturating_duration_since(*at) < TRACKING_STALE)
            .map(|(_, tracking)| tracking)
    }
}

/// Every camera the vehicle's components have described.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cameras {
    cameras: BTreeMap<CameraId, Camera>,
}

impl Cameras {
    pub fn get(&self, id: CameraId) -> Option<&Camera> {
        self.cameras.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (CameraId, &Camera)> {
        self.cameras.iter().map(|(id, camera)| (*id, camera))
    }

    /// The camera a picture's tracking goes to: the first that can track at
    /// all, by component and device.
    pub fn tracker(&self) -> Option<(CameraId, &Camera)> {
        self.iter().find(|(_, camera)| camera.tracking.any())
    }

    pub(crate) fn observe_information(
        &mut self,
        component: u8,
        message: &messages::CameraInformation,
    ) {
        let id = CameraId {
            component,
            device: message.camera_device_id,
        };
        let tracking = TrackingSupport {
            point: message.flags.contains(CameraCapFlags::HAS_TRACKING_POINT),
            rectangle: message
                .flags
                .contains(CameraCapFlags::HAS_TRACKING_RECTANGLE),
        };
        let camera = self.cameras.entry(id).or_insert_with(|| Camera {
            vendor: None,
            model: None,
            tracking,
            report: None,
        });
        camera.vendor = string_from_cstring(&message.vendor_name);
        camera.model = string_from_cstring(&message.model_name);
        camera.tracking = tracking;
    }

    /// Fold a tracking report. One from a camera that has not described
    /// itself is dropped: nothing could act on it.
    pub(crate) fn observe_tracking(
        &mut self,
        time: Instant,
        component: u8,
        message: &messages::CameraTrackingImageStatus,
    ) {
        let id = CameraId {
            component,
            device: message.camera_device_id,
        };
        let Some(camera) = self.cameras.get_mut(&id) else {
            return;
        };
        camera.report = Some((time, tracking_of(message)));
    }
}

fn tracking_of(message: &messages::CameraTrackingImageStatus) -> Tracking {
    match message.tracking_status {
        CameraTrackingStatusFlags::Idle => Tracking::Idle,
        CameraTrackingStatusFlags::Error => Tracking::Error,
        CameraTrackingStatusFlags::Active => Tracking::Active(target_of(message)),
    }
}

/// Where the report puts the target; `NaN` fields mean it does not say.
fn target_of(message: &messages::CameraTrackingImageStatus) -> Option<Target> {
    match message.tracking_mode {
        CameraTrackingMode::None => None,
        CameraTrackingMode::Point => Some(Target::Point {
            at: ImagePoint::new(message.point_x, message.point_y)?,
            radius: Some(message.radius).filter(|radius| radius.is_finite() && *radius >= 0.0),
        }),
        CameraTrackingMode::Rectangle => Some(Target::Rectangle(ImageRect::spanning(
            ImagePoint::new(message.rec_top_x, message.rec_top_y)?,
            ImagePoint::new(message.rec_bottom_x, message.rec_bottom_y)?,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn information(device: u8, flags: CameraCapFlags) -> messages::CameraInformation {
        let mut model_name = [0; 32];
        model_name[..4].copy_from_slice(b"Cam1");
        messages::CameraInformation {
            flags,
            camera_device_id: device,
            model_name,
            ..Default::default()
        }
    }

    fn rectangle_report(device: u8) -> messages::CameraTrackingImageStatus {
        messages::CameraTrackingImageStatus {
            tracking_status: CameraTrackingStatusFlags::Active,
            tracking_mode: CameraTrackingMode::Rectangle,
            point_x: f32::NAN,
            point_y: f32::NAN,
            radius: f32::NAN,
            rec_top_x: 0.25,
            rec_top_y: 0.5,
            rec_bottom_x: 0.75,
            rec_bottom_y: 0.875,
            camera_device_id: device,
            ..Default::default()
        }
    }

    #[test]
    fn information_says_what_a_camera_can_track() {
        let mut cameras = Cameras::default();
        cameras.observe_information(100, &information(0, CameraCapFlags::HAS_TRACKING_POINT));

        let id = CameraId {
            component: 100,
            device: 0,
        };
        let camera = cameras.get(id).expect("described");
        assert_eq!(camera.model.as_deref(), Some("Cam1"));
        assert_eq!(
            camera.tracking,
            TrackingSupport {
                point: true,
                rectangle: false
            }
        );
        assert_eq!(cameras.tracker().map(|(id, _)| id), Some(id));
    }

    #[test]
    fn a_camera_that_cannot_track_is_no_tracker() {
        let mut cameras = Cameras::default();
        cameras.observe_information(1, &information(1, CameraCapFlags::CAPTURE_VIDEO));
        cameras.observe_information(1, &information(2, CameraCapFlags::HAS_TRACKING_RECTANGLE));

        let (id, _) = cameras.tracker().expect("the second tracks");
        assert_eq!(
            id,
            CameraId {
                component: 1,
                device: 2
            }
        );
    }

    #[test]
    fn a_report_says_where_the_target_is_until_it_is_stale() {
        let mut cameras = Cameras::default();
        cameras.observe_information(1, &information(1, CameraCapFlags::HAS_TRACKING_RECTANGLE));
        let at = Instant::now();
        cameras.observe_tracking(at, 1, &rectangle_report(1));

        let camera = cameras
            .get(CameraId {
                component: 1,
                device: 1,
            })
            .expect("described");
        let expected = Tracking::Active(Some(Target::Rectangle(ImageRect {
            top_left: ImagePoint { x: 0.25, y: 0.5 },
            bottom_right: ImagePoint { x: 0.75, y: 0.875 },
        })));
        assert_eq!(camera.tracking(at), Some(expected));
        assert_eq!(camera.tracking(at + TRACKING_STALE), None);
    }

    #[test]
    fn a_camera_reporting_once_a_second_late_keeps_tracking() {
        let mut cameras = Cameras::default();
        cameras.observe_information(1, &information(1, CameraCapFlags::HAS_TRACKING_RECTANGLE));
        let at = Instant::now();
        cameras.observe_tracking(at, 1, &rectangle_report(1));

        let camera = cameras.tracker().expect("a tracker").1;
        let next_report = at + Duration::from_secs(2);
        assert!(camera.tracking(next_report).is_some());
    }

    #[test]
    fn a_report_without_a_place_still_says_it_tracks() {
        let mut cameras = Cameras::default();
        cameras.observe_information(100, &information(0, CameraCapFlags::HAS_TRACKING_POINT));
        let at = Instant::now();
        let report = messages::CameraTrackingImageStatus {
            tracking_status: CameraTrackingStatusFlags::Active,
            tracking_mode: CameraTrackingMode::Point,
            point_x: f32::NAN,
            point_y: f32::NAN,
            ..Default::default()
        };
        cameras.observe_tracking(at, 100, &report);

        let camera = cameras.tracker().expect("a tracker").1;
        assert_eq!(camera.tracking(at), Some(Tracking::Active(None)));
    }

    #[test]
    fn a_report_from_an_undescribed_camera_is_dropped() {
        let mut cameras = Cameras::default();
        cameras.observe_tracking(Instant::now(), 1, &rectangle_report(3));
        assert_eq!(cameras.iter().count(), 0);
    }

    #[test]
    fn a_rectangle_spans_its_corners_in_any_order() {
        let a = ImagePoint { x: 0.75, y: 0.25 };
        let b = ImagePoint { x: 0.25, y: 0.5 };
        assert_eq!(
            ImageRect::spanning(a, b),
            ImageRect {
                top_left: ImagePoint { x: 0.25, y: 0.25 },
                bottom_right: ImagePoint { x: 0.75, y: 0.5 },
            }
        );
        assert_eq!(ImagePoint::new(1.5, 0.5), None);
        assert_eq!(ImagePoint::new(f32::NAN, 0.5), None);
    }
}
