//! Vehicle attitude.

use mavio::default_dialect::messages;
use nalgebra::{Quaternion, UnitQuaternion, Vector3};

#[derive(Debug, Clone, Default)]
pub struct AttitudeState {
    pub current: Option<Attitude>,
}

#[derive(Clone, Debug)]
pub struct Attitude {
    pub attitude: UnitQuaternion<f32>,

    // TODO: Do something with this information
    #[allow(unused)]
    pub angular_rate: Vector3<f32>,
}

impl AttitudeState {
    pub(crate) fn observe_attitude(&mut self, message: &messages::Attitude) {
        // MAVLink ATTITUDE reports roll/pitch/yaw in radians in the NED frame using the
        // aerospace 3-2-1 (Z-Y-X) convention, matching `from_euler_angles`.
        self.current = Some(Attitude {
            attitude: UnitQuaternion::from_euler_angles(message.roll, message.pitch, message.yaw),
            angular_rate: Vector3::new(message.rollspeed, message.pitchspeed, message.yawspeed),
        });
    }

    pub(crate) fn observe_attitude_quaternion(&mut self, message: &messages::AttitudeQuaternion) {
        // MAVLink ATTITUDE_QUATERNION order is (q1..q4) = (w, x, y, z) (Hamilton),
        // matching nalgebra's `Quaternion::new(w, i, j, k)`.
        self.current = Some(Attitude {
            attitude: UnitQuaternion::from_quaternion(Quaternion::new(
                message.q1, message.q2, message.q3, message.q4,
            )),
            angular_rate: Vector3::new(message.rollspeed, message.pitchspeed, message.yawspeed),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_1_SQRT_2, PI};

    fn assert_angles(attitude: &Attitude, expected: (f32, f32, f32)) {
        let (roll, pitch, yaw) = attitude.attitude.euler_angles();
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
        state.observe_attitude_quaternion(&message);

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (0.0, 0.0, 0.0),
        );
    }

    #[test]
    fn attitude_quaternion_decodes_yaw_with_wx_yz_order() {
        // Pure 90 deg yaw about z: (w, x, y, z) = (cos45, 0, 0, sin45). The old code
        // read w from q4, so this case exposed the component shuffle.
        let message = messages::AttitudeQuaternion {
            q1: FRAC_1_SQRT_2,
            q4: FRAC_1_SQRT_2,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude_quaternion(&message);

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
        state.observe_attitude_quaternion(&message);

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (PI / 2.0, 0.0, 0.0),
        );
    }

    #[test]
    fn attitude_euler_decodes_roll() {
        let message = messages::Attitude {
            roll: PI / 2.0,
            ..Default::default()
        };

        let mut state = AttitudeState::default();
        state.observe_attitude(&message);

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
        state.observe_attitude(&message);

        assert_angles(
            state.current.as_ref().expect("attitude recorded"),
            (0.0, 0.0, PI / 2.0),
        );
    }
}
