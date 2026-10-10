//! MAVLink mission model.
//!
//! This module holds the typed command/item representation and its lossless
//! conversion to and from the wire. The mission list itself is a separate
//! concern and lives elsewhere.

// The command model is not wired into the application yet; it is exercised by
// its own tests until the mission list and vehicle integration land.
#![allow(dead_code)]

pub mod command;

#[allow(unused_imports)] // Facade re-export, consumed once the mission list lands.
pub use command::*;
