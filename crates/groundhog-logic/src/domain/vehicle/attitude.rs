//! Vehicle attitude.

use std::time::Instant;

use mavio::default_dialect::messages;
use nalgebra::{Quaternion, UnitQuaternion};

/// The norm below which a reported quaternion means "attitude unknown".
const UNKNOWN_NORM: f32 = 1e-6;

#[derive(Debug, Clone, Default)]
pub struct AttitudeState {
    pub current: Option<Attitude>,
}

/// A vehicle's orientation, always a finite rotation: a reported NaN or
/// infinity has no direction, and drawn it would fail tessellation, which
/// iced's GPU renderer turns into a crash.
#[derive(Clone, Debug)]
pub struct Attitude {
    attitude: UnitQuaternion<f32>,
    /// When the report arrived.
    pub at: Instant,
    /// When the autopilot measured it, by its clock since boot: the spacing
    /// of reports as taken, which their arrival blurs.
    pub time_boot_ms: u32,
}

impl Attitude {
    /// `attitude`, if every component is finite.
    fn new(attitude: UnitQuaternion<f32>, at: Instant, time_boot_ms: u32) -> Option<Self> {
        attitude
            .coords
            .iter()
            .all(|component| component.is_finite())
            .then_some(Self {
                attitude,
                at,
                time_boot_ms,
            })
    }

    /// Roll, pitch and yaw in radians, in the aerospace 3-2-1 convention.
    pub fn euler_angles(&self) -> (f32, f32, f32) {
        self.attitude.euler_angles()
    }
}

impl AttitudeState {
    pub(crate) fn observe_attitude(&mut self, message: &messages::Attitude, at: Instant) {
        // MAVLink ATTITUDE reports roll/pitch/yaw in radians in the NED frame using the
        // aerospace 3-2-1 (Z-Y-X) convention, matching `from_euler_angles`.
        self.current = Attitude::new(
            UnitQuaternion::from_euler_angles(message.roll, message.pitch, message.yaw),
            at,
            message.time_boot_ms,
        );
    }

    pub(crate) fn observe_attitude_quaternion(
        &mut self,
        message: &messages::AttitudeQuaternion,
        at: Instant,
    ) {
        // MAVLink ATTITUDE_QUATERNION order is (q1..q4) = (w, x, y, z) (Hamilton),
        // matching nalgebra's `Quaternion::new(w, i, j, k)`.
        let quaternion = Quaternion::new(message.q1, message.q2, message.q3, message.q4);

        // All zeros is how a sender says it does not know the attitude; it has
        // no direction to normalise to.
        let norm = quaternion.norm();
        self.current = (norm.is_finite() && norm > UNKNOWN_NORM)
            .then(|| {
                Attitude::new(
                    UnitQuaternion::from_quaternion(quaternion),
                    at,
                    message.time_boot_ms,
                )
            })
            .flatten();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_1_SQRT_2, PI};

    fn assert_angles(attitude: &Attitude, expected: (f32, f32, f32)) {
        let (roll, pitch, yaw) = attitude.euler_angles();
        assert!((roll - expected.0).abs() < 1e-6, "roll: {roll}");
        assert!((pitch - expected.1).abs() < 1e-6, "pitch: {pitch}");
        assert!((yaw - expected.2).abs() < 1e-6, "yaw: {yaw}");
    }

    #[test]
    fn attitude_quaternion_identity_is_zero_rotation() {
        let message = messages::AttitudeQuaternion {
            q1: 1.0,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude_quaternion(&message, Instant::now());

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (0.0, 0.0, 0.0),
        );
    }

    #[test]
    fn attitude_quaternion_decodes_yaw_with_wx_yz_order() {
        // Pure 90 deg yaw about z: (w, x, y, z) = (cos45, 0, 0, sin45). Reading
        // w from another component would turn it about another axis.
        let message = messages::AttitudeQuaternion {
            q1: FRAC_1_SQRT_2,
            q4: FRAC_1_SQRT_2,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude_quaternion(&message, Instant::now());

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (0.0, 0.0, PI / 2.0),
        );
    }

    #[test]
    fn attitude_quaternion_decodes_roll_about_x() {
        // Pure 90 deg roll about x: (w, x, y, z) = (cos45, sin45, 0, 0).
        let message = messages::AttitudeQuaternion {
            q1: FRAC_1_SQRT_2,
            q2: FRAC_1_SQRT_2,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude_quaternion(&message, Instant::now());

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (PI / 2.0, 0.0, 0.0),
        );
    }

    #[test]
    fn an_attitude_keeps_when_it_was_measured_and_arrived() {
        let at = Instant::now();
        let mut state = AttitudeState::default();
        state.observe_attitude(
            &messages::Attitude {
                time_boot_ms: 1234,
                ..Default::default()
            },
            at,
        );
        let attitude = state.current.as_ref().expect("attitude recorded");
        assert_eq!((attitude.at, attitude.time_boot_ms), (at, 1234));

        state.observe_attitude_quaternion(
            &messages::AttitudeQuaternion {
                q1: 1.0,
                time_boot_ms: 1250,
                ..Default::default()
            },
            at,
        );
        let attitude = state.current.as_ref().expect("attitude recorded");
        assert_eq!(attitude.time_boot_ms, 1250);
    }

    #[test]
    fn attitude_euler_decodes_roll() {
        let message = messages::Attitude {
            roll: PI / 2.0,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude(&message, Instant::now());

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (PI / 2.0, 0.0, 0.0),
        );
    }

    #[test]
    fn attitude_euler_decodes_yaw() {
        let message = messages::Attitude {
            yaw: PI / 2.0,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude(&message, Instant::now());

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (0.0, 0.0, PI / 2.0),
        );
    }

    /// A non-finite angle has no direction; drawn, it would crash the
    /// renderer. The attitude is unknown until a finite one arrives.
    #[test]
    fn non_finite_euler_angles_are_an_unknown_attitude() {
        let mut state = AttitudeState::default();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for message in [
                messages::Attitude {
                    roll: bad,
                    ..Default::default()
                },
                messages::Attitude {
                    pitch: bad,
                    ..Default::default()
                },
                messages::Attitude {
                    yaw: bad,
                    ..Default::default()
                },
            ] {
                state.observe_attitude(
                    &messages::Attitude {
                        yaw: 1.0,
                        ..Default::default()
                    },
                    Instant::now(),
                );
                state.observe_attitude(&message, Instant::now());
                assert!(state.current.is_none(), "{message:?}");
            }
        }

        state.observe_attitude(
            &messages::Attitude {
                yaw: PI / 2.0,
                ..Default::default()
            },
            Instant::now(),
        );
        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (0.0, 0.0, PI / 2.0),
        );
    }

    #[test]
    fn non_finite_quaternions_are_an_unknown_attitude() {
        let mut state = AttitudeState::default();
        for bad in [f32::NAN, f32::INFINITY] {
            state.observe_attitude_quaternion(
                &messages::AttitudeQuaternion {
                    q1: 1.0,
                    ..Default::default()
                },
                Instant::now(),
            );
            state.observe_attitude_quaternion(
                &messages::AttitudeQuaternion {
                    q1: 1.0,
                    q4: bad,
                    ..Default::default()
                },
                Instant::now(),
            );
            assert!(state.current.is_none(), "q4 = {bad}");
        }
    }

    #[test]
    fn an_all_zero_quaternion_is_an_unknown_attitude() {
        let mut state = AttitudeState::default();
        state.observe_attitude_quaternion(
            &messages::AttitudeQuaternion {
                q1: 1.0,
                ..Default::default()
            },
            Instant::now(),
        );
        state.observe_attitude_quaternion(&messages::AttitudeQuaternion::default(), Instant::now());

        assert!(state.current.is_none(), "unknown, not NaN");
    }
}
