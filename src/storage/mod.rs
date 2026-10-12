//! External file formats for domain types.
//!
//! This module owns the on-disk representations that domain types are read from
//! and written to, such as the parameter INI grammar and the "QGC WPL 110"
//! mission format. Keeping them here leaves the domain free of persistence
//! concerns: the domain exposes state and typed conversions, while the format
//! grammars, error types, and version quirks live in this module.

pub mod mission;
pub mod parameter;
