//! Vehicle attitude and angular rates.

use std::time::Instant;

use mavio::default_dialect::messages;
use nalgebra::{Quaternion, UnitQuaternion, Vector3};

#[derive(Debug, Clone, Default)]
pub struct AttitudeState {
    pub current: Option<Attitude>,
    pub angular_rates: Option<AngularRate>,
}

#[derive(Clone, Debug)]
pub struct Attitude {
    pub at: Instant,
    pub attitude: UnitQuaternion<f32>,
}

#[derive(Clone, Debug)]
pub struct AngularRate {
    pub at: Instant,
    pub rates: Vector3<f32>,
}

impl AttitudeState {
    pub fn observe_attitude(&mut self, message: &messages::Attitude, time: Instant) {
        self.angular_rates = Some(AngularRate {
            at: time,
            rates: Vector3::new(message.rollspeed, message.pitchspeed, message.yawspeed),
        });

        // TODO: Check order of rotation angles
        self.current = Some(Attitude {
            at: time,
            attitude: UnitQuaternion::from_euler_angles(message.roll, message.pitch, message.yaw),
        });
    }

    pub fn observe_attitude_quaternion(
        &mut self,
        message: &messages::AttitudeQuaternion,
        time: Instant,
    ) {
        self.angular_rates = Some(AngularRate {
            at: time,
            rates: Vector3::new(message.rollspeed, message.pitchspeed, message.yawspeed),
        });

        // TODO: Check order quaternion scalars
        self.current = Some(Attitude {
            at: time,
            attitude: UnitQuaternion::from_quaternion(Quaternion::new(
                message.q4, message.q1, message.q2, message.q3,
            )),
        });
    }
}
