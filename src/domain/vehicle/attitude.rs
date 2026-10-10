//! Vehicle attitude.

use mavio::default_dialect::messages;
use nalgebra::{Quaternion, UnitQuaternion};

#[derive(Debug, Clone, Default)]
pub struct AttitudeState {
    pub current: Option<Attitude>,
}

#[derive(Clone, Debug)]
pub struct Attitude {
    pub attitude: UnitQuaternion<f32>,
}

impl AttitudeState {
    pub fn observe_attitude(&mut self, message: &messages::Attitude) {
        // TODO: Check order of rotation angles
        self.current = Some(Attitude {
            attitude: UnitQuaternion::from_euler_angles(message.roll, message.pitch, message.yaw),
        });
    }

    pub fn observe_attitude_quaternion(&mut self, message: &messages::AttitudeQuaternion) {
        // TODO: Check order quaternion scalars
        self.current = Some(Attitude {
            attitude: UnitQuaternion::from_quaternion(Quaternion::new(
                message.q4, message.q1, message.q2, message.q3,
            )),
        });
    }
}
