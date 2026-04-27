//! Import parsers for third-party brush formats.
//!
//! Each format gets its own submodule with a typed AST and a `parse_*` entry
//! point. Currently only Krita (`.kpp`) presets are supported. Output is a
//! debug-friendly representation — conversion into Darkly's native brush graph
//! is a separate later step driven by what we learn from real-world brushes.

pub mod krita;
