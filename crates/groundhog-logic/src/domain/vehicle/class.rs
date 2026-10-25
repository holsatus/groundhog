//! What kind of machine a heartbeat describes.
//!
//! `MAV_TYPE` lists dozens of kinds, many of which are not vehicles at all
//! (ground stations, gimbals, cameras, ADS-B receivers, ...). The fleet only
//! registers a system as a vehicle once one of its components heartbeats as a
//! vehicle kind, and firmware adapters pick their mode tables by class rather
//! than by individual frame type, so a hexacopter and a quadcopter share the
//! copter modes.

use mavio::default_dialect::enums::MavType;

/// The broad class of a vehicle, or [`VehicleClass::NotAVehicle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VehicleClass {
    /// Any multirotor or helicopter.
    Copter,
    /// A fixed-wing aircraft without vertical lift.
    FixedWing,
    /// A fixed-wing aircraft with vertical take-off and landing.
    Vtol,
    Rover,
    Boat,
    Submarine,
    Airship,
    AntennaTracker,
    /// A vehicle of a kind this GCS has no specific support for.
    #[default]
    Other,
    /// A component that is not a vehicle: a GCS, gimbal, camera, ...
    NotAVehicle,
}

impl VehicleClass {
    pub fn of(mav_type: MavType) -> Self {
        use MavType as T;

        match mav_type {
            T::Quadrotor
            | T::Hexarotor
            | T::Octorotor
            | T::Tricopter
            | T::Coaxial
            | T::Helicopter
            | T::Dodecarotor
            | T::Decarotor
            | T::GenericMultirotor => VehicleClass::Copter,
            T::FixedWing | T::FlappingWing | T::Kite | T::Parafoil => VehicleClass::FixedWing,
            T::VtolTailsitterDuorotor
            | T::VtolTailsitterQuadrotor
            | T::VtolTiltrotor
            | T::VtolFixedrotor
            | T::VtolTailsitter
            | T::VtolTiltwing
            | T::VtolReserved5
            | T::VtolGyrodyne => VehicleClass::Vtol,
            T::GroundRover | T::GroundQuadruped => VehicleClass::Rover,
            T::SurfaceBoat => VehicleClass::Boat,
            T::Submarine => VehicleClass::Submarine,
            T::Airship | T::FreeBalloon => VehicleClass::Airship,
            T::AntennaTracker => VehicleClass::AntennaTracker,
            T::Generic | T::Rocket | T::SpacecraftOrbiter => VehicleClass::Other,
            T::Gcs
            | T::OnboardController
            | T::Gimbal
            | T::Adsb
            | T::Camera
            | T::ChargingStation
            | T::Flarm
            | T::Servo
            | T::Odid
            | T::Battery
            | T::Parachute
            | T::Log
            | T::Osd
            | T::Imu
            | T::Gps
            | T::Winch
            | T::Illuminator => VehicleClass::NotAVehicle,
        }
    }

    pub fn is_vehicle(self) -> bool {
        self != VehicleClass::NotAVehicle
    }

    /// Whether the vehicle flies on wings, and so may carry an airspeed
    /// sensor.
    pub fn has_wings(self) -> bool {
        matches!(self, VehicleClass::FixedWing | VehicleClass::Vtol)
    }

    pub fn name(self) -> &'static str {
        match self {
            VehicleClass::Copter => "Copter",
            VehicleClass::FixedWing => "Plane",
            VehicleClass::Vtol => "VTOL",
            VehicleClass::Rover => "Rover",
            VehicleClass::Boat => "Boat",
            VehicleClass::Submarine => "Submarine",
            VehicleClass::Airship => "Airship",
            VehicleClass::AntennaTracker => "Antenna tracker",
            VehicleClass::Other => "Vehicle",
            VehicleClass::NotAVehicle => "Component",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_multirotor_frame_is_a_copter() {
        for frame in [
            MavType::Quadrotor,
            MavType::Hexarotor,
            MavType::Octorotor,
            MavType::Tricopter,
            MavType::Coaxial,
            MavType::Helicopter,
            MavType::Dodecarotor,
        ] {
            assert_eq!(VehicleClass::of(frame), VehicleClass::Copter, "{frame:?}");
        }
    }

    #[test]
    fn ground_stations_and_peripherals_are_not_vehicles() {
        for kind in [
            MavType::Gcs,
            MavType::Gimbal,
            MavType::Camera,
            MavType::Adsb,
            MavType::OnboardController,
        ] {
            assert!(!VehicleClass::of(kind).is_vehicle(), "{kind:?}");
        }
    }
}
