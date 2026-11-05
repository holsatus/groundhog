//! Vehicle state and the rules about it, free of I/O.
//!
//! [`fleet::Fleet`] folds decoded messages into [`vehicle::Vehicle`]s and
//! publishes immutable [`fleet::FleetSnapshot`]s, which is all a frontend ever
//! reads. Missions, parameters, firmware adapters and calibration protocols
//! give that state its meaning. Folding is crate-private: only the fleet
//! runtime advances vehicle state.

pub mod calibration;
pub mod firmware;
pub mod fleet;
pub mod geo;
pub mod mission;
pub mod parameter;
pub mod vehicle;
