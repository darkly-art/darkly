//! Registry projection: every modular registry in one browsable shape.

use darkly_macros::handlers;

use super::DarklyEngine;
use crate::catalog::Catalog;

#[handlers]
impl DarklyEngine {
    /// Every registry, projected into the one browsable shape the UI pickers,
    /// the settings surface and the metadata export all consume. Delegates to
    /// the GPU-free free function so an exporter can build the same data
    /// without an engine; the handler exists so `ts_rs` emits `Catalog` into
    /// the frontend's typed client.
    #[handler]
    pub fn catalogs(&self) -> Vec<Catalog> {
        crate::catalog::catalogs()
    }
}
