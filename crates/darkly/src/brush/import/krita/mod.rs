//! Parsers for Krita brush resources.
//!
//! Entry point: [`kpp::parse_kpp`] for `.kpp` preset files. The resulting
//! [`kpp::KritaPreset`] holds every PNG chunk, every preset XML param (with
//! best-effort decoded value), and every embedded resource (raw bytes plus
//! magic-byte sniffed format label).
//!
//! Format reference: ground-truthed against
//! `krita/libs/image/brushengine/kis_paintop_preset.cpp` and
//! `krita/libs/image/kis_properties_configuration.cc`. See also
//! `docs/brush/krita-brush-system.md` for the high-level overview.

pub mod kpp;
pub mod paintop;
pub mod resource;
pub mod sensor;
pub mod xml;

pub use kpp::{parse_kpp, KritaPreset, ParseError};
