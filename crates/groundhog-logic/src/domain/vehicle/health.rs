//! What a vehicle reports of its own health: its sensors and subsystems
//! (`SYS_STATUS`), and its GPS (`GPS_RAW_INT`).
//!
//! `SYS_STATUS` carries three masks over the same flags: which sensors are
//! present, which are enabled, and which are healthy. Duplicated sensors
//! (a second gyro, accelerometer or compass) are folded into one entry,
//! healthy only while every copy present is.

use mavio::default_dialect::{
    enums::{GpsFixType, MavSysStatusSensor},
    messages,
};

/// A sensor or subsystem a vehicle reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sensor {
    Gyro,
    Accelerometer,
    Compass,
    Barometer,
    Airspeed,
    Gps,
    OpticalFlow,
    Rangefinder,
    RcReceiver,
    Ahrs,
    Terrain,
    Motors,
    Battery,
    Logging,
    Geofence,
    Proximity,
}

impl Sensor {
    /// A short name, for labels.
    pub fn name(self) -> &'static str {
        match self {
            Sensor::Gyro => "Gyro",
            Sensor::Accelerometer => "Accel",
            Sensor::Compass => "Compass",
            Sensor::Barometer => "Baro",
            Sensor::Airspeed => "Airspeed",
            Sensor::Gps => "GPS",
            Sensor::OpticalFlow => "Flow",
            Sensor::Rangefinder => "Range",
            Sensor::RcReceiver => "RC",
            Sensor::Ahrs => "AHRS",
            Sensor::Terrain => "Terrain",
            Sensor::Motors => "Motors",
            Sensor::Battery => "Battery",
            Sensor::Logging => "Logging",
            Sensor::Geofence => "Fence",
            Sensor::Proximity => "Proximity",
        }
    }

    /// The `SYS_STATUS` flags of the sensor, in the order sensors are listed.
    fn flags() -> [(Sensor, MavSysStatusSensor); 16] {
        use MavSysStatusSensor as F;
        [
            (Sensor::Gyro, F::_3D_GYRO.union(F::_3D_GYRO2)),
            (Sensor::Accelerometer, F::_3D_ACCEL.union(F::_3D_ACCEL2)),
            (Sensor::Compass, F::_3D_MAG.union(F::_3D_MAG2)),
            (Sensor::Barometer, F::ABSOLUTE_PRESSURE),
            (Sensor::Airspeed, F::DIFFERENTIAL_PRESSURE),
            (Sensor::Gps, F::GPS),
            (Sensor::OpticalFlow, F::OPTICAL_FLOW),
            (Sensor::Rangefinder, F::LASER_POSITION),
            (Sensor::RcReceiver, F::RC_RECEIVER),
            (Sensor::Ahrs, F::MAV_SYS_STATUS_AHRS),
            (Sensor::Terrain, F::MAV_SYS_STATUS_TERRAIN),
            (Sensor::Motors, F::MOTOR_OUTPUTS.union(F::PROPULSION)),
            (Sensor::Battery, F::BATTERY),
            (Sensor::Logging, F::MAV_SYS_STATUS_LOGGING),
            (Sensor::Geofence, F::MAV_SYS_STATUS_GEOFENCE),
            (Sensor::Proximity, F::PROXIMITY),
        ]
    }
}

/// How one present sensor is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorStatus {
    pub sensor: Sensor,
    /// Whether the vehicle uses it. A disabled sensor's health says nothing.
    pub enabled: bool,
    pub healthy: bool,
}

/// What the vehicle's GPS reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gps {
    pub fix: GpsFixType,
    /// Satellites in view, if known.
    pub satellites: Option<u8>,
    /// Horizontal dilution of precision, if known: below 1 is excellent,
    /// above 2 poor.
    pub hdop: Option<f32>,
}

impl Gps {
    /// Whether the fix gives a 3D position.
    pub fn has_position(&self) -> bool {
        !matches!(
            self.fix,
            GpsFixType::NoGps | GpsFixType::NoFix | GpsFixType::_2dFix
        )
    }

    /// The fix, for labels.
    pub fn fix_name(&self) -> &'static str {
        match self.fix {
            GpsFixType::NoGps => "No GPS",
            GpsFixType::NoFix => "No fix",
            GpsFixType::_2dFix => "2D fix",
            GpsFixType::_3dFix => "3D fix",
            GpsFixType::Dgps => "DGPS",
            GpsFixType::RtkFloat => "RTK float",
            GpsFixType::RtkFixed => "RTK fixed",
            GpsFixType::Static => "Static",
            GpsFixType::Ppp => "PPP",
        }
    }
}

/// A vehicle's health, as it reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Health {
    /// The present, enabled and healthy masks of the last `SYS_STATUS`.
    masks: Option<[MavSysStatusSensor; 3]>,
    gps: Option<Gps>,
}

impl Health {
    /// Every sensor the vehicle reports as present, in a fixed order.
    /// Empty until it reports.
    pub fn sensors(&self) -> Vec<SensorStatus> {
        let Some([present, enabled, healthy]) = self.masks else {
            return Vec::new();
        };

        Sensor::flags()
            .into_iter()
            .filter_map(|(sensor, flags)| {
                let flags = flags.intersection(present);
                (!flags.is_empty()).then(|| SensorStatus {
                    sensor,
                    enabled: enabled.intersects(flags),
                    healthy: healthy.contains(flags),
                })
            })
            .collect()
    }

    /// The sensors in use that are not healthy.
    pub fn failing(&self) -> impl Iterator<Item = Sensor> {
        self.sensors()
            .into_iter()
            .filter(|status| status.enabled && !status.healthy)
            .map(|status| status.sensor)
    }

    /// Whether the vehicle's pre-arm checks pass; `None` when it does not
    /// report them. (They always pass while armed.)
    pub fn prearm_passes(&self) -> Option<bool> {
        let [present, _, healthy] = self.masks?;
        let flag = MavSysStatusSensor::MAV_SYS_STATUS_PREARM_CHECK;
        present.contains(flag).then(|| healthy.contains(flag))
    }

    pub fn gps(&self) -> Option<&Gps> {
        self.gps.as_ref()
    }

    pub(crate) fn observe_sys_status(&mut self, message: &messages::SysStatus) {
        self.masks = Some([
            message.onboard_control_sensors_present,
            message.onboard_control_sensors_enabled,
            message.onboard_control_sensors_health,
        ]);
    }

    pub(crate) fn observe_gps_raw_int(&mut self, message: &messages::GpsRawInt) {
        self.gps = Some(Gps {
            fix: message.fix_type,
            satellites: (message.satellites_visible != u8::MAX)
                .then_some(message.satellites_visible),
            hdop: (message.eph != u16::MAX).then(|| message.eph as f32 / 100.0),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use MavSysStatusSensor as F;

    fn sys_status(
        present: MavSysStatusSensor,
        enabled: MavSysStatusSensor,
        healthy: MavSysStatusSensor,
    ) -> messages::SysStatus {
        messages::SysStatus {
            onboard_control_sensors_present: present,
            onboard_control_sensors_enabled: enabled,
            onboard_control_sensors_health: healthy,
            ..Default::default()
        }
    }

    #[test]
    fn sensors_are_listed_as_present_enabled_and_healthy() {
        let mut health = Health::default();
        assert!(health.sensors().is_empty(), "nothing before a report");

        let present = F::_3D_GYRO | F::_3D_MAG | F::GPS | F::DIFFERENTIAL_PRESSURE;
        let enabled = F::_3D_GYRO | F::_3D_MAG | F::GPS;
        let healthy = F::_3D_GYRO | F::GPS | F::DIFFERENTIAL_PRESSURE;
        health.observe_sys_status(&sys_status(present, enabled, healthy));

        assert_eq!(
            health.sensors(),
            [
                SensorStatus {
                    sensor: Sensor::Gyro,
                    enabled: true,
                    healthy: true
                },
                SensorStatus {
                    sensor: Sensor::Compass,
                    enabled: true,
                    healthy: false
                },
                SensorStatus {
                    sensor: Sensor::Airspeed,
                    enabled: false,
                    healthy: true
                },
                SensorStatus {
                    sensor: Sensor::Gps,
                    enabled: true,
                    healthy: true
                },
            ]
        );
        assert_eq!(health.failing().collect::<Vec<_>>(), [Sensor::Compass]);
    }

    #[test]
    fn a_duplicated_sensor_is_healthy_only_while_every_copy_is() {
        let mut health = Health::default();
        let both = F::_3D_ACCEL | F::_3D_ACCEL2;
        health.observe_sys_status(&sys_status(both, both, F::_3D_ACCEL));

        let accel = health.sensors()[0];
        assert_eq!(accel.sensor, Sensor::Accelerometer);
        assert!(!accel.healthy, "the second accelerometer is not");
    }

    #[test]
    fn prearm_is_reported_only_when_the_vehicle_reports_it() {
        let mut health = Health::default();
        health.observe_sys_status(&sys_status(F::GPS, F::GPS, F::GPS));
        assert_eq!(health.prearm_passes(), None);

        let prearm = F::MAV_SYS_STATUS_PREARM_CHECK;
        health.observe_sys_status(&sys_status(prearm, prearm, F::empty()));
        assert_eq!(health.prearm_passes(), Some(false));
        health.observe_sys_status(&sys_status(prearm, prearm, prearm));
        assert_eq!(health.prearm_passes(), Some(true));
    }

    #[test]
    fn gps_sentinels_mean_unknown() {
        let mut health = Health::default();
        health.observe_gps_raw_int(&messages::GpsRawInt {
            fix_type: GpsFixType::_3dFix,
            satellites_visible: u8::MAX,
            eph: u16::MAX,
            ..Default::default()
        });
        let gps = health.gps().unwrap();
        assert!(gps.has_position());
        assert_eq!((gps.satellites, gps.hdop), (None, None));

        health.observe_gps_raw_int(&messages::GpsRawInt {
            fix_type: GpsFixType::_2dFix,
            satellites_visible: 7,
            eph: 180,
            ..Default::default()
        });
        let gps = health.gps().unwrap();
        assert!(!gps.has_position());
        assert_eq!((gps.satellites, gps.hdop), (Some(7), Some(1.8)));
    }
}
