//! Wire-format I/O for modular instances.
//!
//! Every modular system (veils, brush nodes, stabilizers, …) shares the
//! same identity-on-wire: `{ "type_id": "<slug>", "params": [...] }`. These
//! helpers materialize that shape from any instance and parse it back. The
//! save/load core never matches on module identity — it just calls
//! [`serialize_instance`] and [`deserialize_instance`].
//!
//! Helpers are intentionally minimal — the registries vary in their
//! constructor signature (veils need a GPU pipeline; stabilizers don't),
//! so per-registry callers stay close. What we centralize is the *shape*,
//! not the dispatch.

use serde::{Deserialize, Serialize};

use super::error::LoadError;
use crate::gpu::params::ParamValue;

/// Canonical wire-format envelope for a modular instance.
///
/// Veils, brush nodes, stabilizers — every `(type_id, params)` pair on
/// disk serializes through this shape. Phase 2 uses it for round-trip
/// shape verification; Phase 3 writes the same shape into the manifest's
/// `veils` array and the brush-graph node list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstancePayload {
    pub type_id: String,
    #[serde(default)]
    pub params: Vec<ParamValue>,
}

impl InstancePayload {
    pub fn new(type_id: impl Into<String>, params: Vec<ParamValue>) -> Self {
        InstancePayload {
            type_id: type_id.into(),
            params,
        }
    }
}

/// Serialize a `(type_id, params)` pair to the canonical
/// [`InstancePayload`] JSON shape. Used by every modular system on the
/// save path.
pub fn serialize_instance(
    type_id: &str,
    params: Vec<ParamValue>,
) -> Result<serde_json::Value, LoadError> {
    serde_json::to_value(InstancePayload::new(type_id, params)).map_err(LoadError::from)
}

/// Deserialize an [`InstancePayload`] from JSON. The caller is responsible
/// for resolving `payload.type_id` against the appropriate registry — this
/// helper only handles the JSON shape, not the registry dispatch.
///
/// Returns [`LoadError::Json`] for malformed input; the registry-miss case
/// surfaces as [`LoadError::UnknownTypeId`] at the call site.
pub fn deserialize_instance(value: &serde_json::Value) -> Result<InstancePayload, LoadError> {
    serde_json::from_value(value.clone()).map_err(LoadError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trips_via_json() {
        let payload =
            InstancePayload::new("noise", vec![ParamValue::Float(0.5), ParamValue::Int(3)]);
        let json = serde_json::to_value(&payload).unwrap();
        let back: InstancePayload = serde_json::from_value(json).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn deserialize_missing_params_defaults_to_empty() {
        let json = serde_json::json!({ "type_id": "noop" });
        let payload = deserialize_instance(&json).unwrap();
        assert_eq!(payload.type_id, "noop");
        assert!(payload.params.is_empty());
    }

    #[test]
    fn deserialize_malformed_returns_json_error() {
        let json = serde_json::json!({ "params": [] }); // missing type_id
        let err = deserialize_instance(&json).unwrap_err();
        assert!(matches!(err, LoadError::Json(_)));
    }
}
