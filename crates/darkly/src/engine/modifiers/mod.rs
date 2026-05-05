//! Engine-side modifier operations.
//!
//! Each modifier kind that needs engine-level helpers (e.g. mask's
//! `apply_mask` baking) lives in its own file here. Generic modifier ops
//! (insert, remove, visibility/lock toggle) ride on the existing layer/node
//! engine helpers and don't need a per-kind file.

pub mod mask;
pub mod selection;
