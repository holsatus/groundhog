//! Opaque link handles.

use std::sync::Arc;

/// An opaque handle identifying one configured link.
///
/// Deliberately independent of the link's display name: renaming a link never
/// changes its identity, and therefore never changes the router attachment it
/// names. Cheap to clone and usable as a map key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(Arc<str>);

impl LinkId {
    pub fn new(id: &str) -> Self {
        LinkId(Arc::from(id))
    }
}
