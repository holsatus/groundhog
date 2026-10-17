//! Pure MAVLink wire codecs shared across layers.
//!
//! This module owns the lossless conversions between typed values and their
//! MAVLink wire representations. It depends only on `mav_param` and `mavio`:
//! the domain folds raw messages into typed state using these codecs, and the
//! middleware uses them to encode outbound parameters, so neither layer has to
//! reach into the other for wire logic.

pub mod param;

/// Scale between WGS-84 degrees and the integer coordinates of
/// `GLOBAL_POSITION_INT`, `HOME_POSITION` and `MISSION_ITEM_INT` (all in
/// degrees × 1e7).
pub const LATLON_SCALE: f64 = 1e-7;
