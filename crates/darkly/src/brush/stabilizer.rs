//! Stroke stabilizer — retroactive stroke reshaping with zero lag.
//!
//! The stabilizer processes the full stroke history before dabs are placed.
//! It operates outside the per-dab node graph: presets configure which
//! algorithm to use and its parameters, and the engine constructs the
//! algorithm at stroke start.
//!
//! Follows the same modular registry pattern as veils (`gpu/veil.rs` +
//! `gpu/veils/*.rs`): each algorithm is a self-contained module that
//! declares its own params and factory.  A registry maps type_id →
//! registration.  New algorithms are added by dropping a `.rs` file in
//! `brush/stabilizers/` — no other files touched.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paint_info::PaintInformation;
use crate::gpu::params::{ParamDef, ParamValue};

/// Result of pushing a new point through the stabilizer.
pub struct StabilizeResult {
    /// Earliest dab index that needs re-rendering (everything from here
    /// to the tip has changed).  `None` means nothing diverged — only
    /// new points were appended.
    pub divergence_index: Option<usize>,
}

/// The trait that all stabilizer algorithms implement.
pub trait StabilizerAlgorithm: Send {
    /// Append a raw input point, run the algorithm, and return the result.
    fn push(&mut self, point: PaintInformation) -> StabilizeResult;

    /// The current stabilized polyline (full stroke).
    fn stabilized(&self) -> &[PaintInformation];

    /// Number of points in the stabilized polyline.
    fn len(&self) -> usize {
        self.stabilized().len()
    }

    /// Reset for a new stroke.
    fn clear(&mut self);
}

/// A pass-through "stabilizer" that does nothing — output equals input.
/// Used when no stabilization is configured (empty algorithm string).
pub struct PassThrough {
    points: Vec<PaintInformation>,
}

impl PassThrough {
    pub fn new() -> Self {
        Self { points: Vec::with_capacity(256) }
    }
}

impl StabilizerAlgorithm for PassThrough {
    fn push(&mut self, point: PaintInformation) -> StabilizeResult {
        self.points.push(point);
        StabilizeResult { divergence_index: None }
    }

    fn stabilized(&self) -> &[PaintInformation] {
        &self.points
    }

    fn clear(&mut self) {
        self.points.clear();
    }
}

/// What each stabilizer module returns from its `register()` function.
pub struct StabilizerRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    pub from_params: fn(&[ParamValue]) -> Box<dyn StabilizerAlgorithm>,
}

/// Auto-discovered stabilizer registry.
pub struct StabilizerRegistry {
    entries: HashMap<&'static str, StabilizerRegistration>,
}

impl StabilizerRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::stabilizers::registrations() {
            entries.insert(reg.type_id, reg);
        }
        StabilizerRegistry { entries }
    }

    /// Return all registered stabilizer type IDs with their parameter definitions.
    pub fn types(&self) -> Vec<(&'static str, &'static str, &'static [ParamDef])> {
        let mut types: Vec<_> = self.entries
            .iter()
            .map(|(&id, reg)| (id, reg.display_name, reg.params))
            .collect();
        types.sort_by_key(|(id, _, _)| *id);
        types
    }

    /// Get the static parameter definitions for a stabilizer type.
    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries
            .get(type_id)
            .map(|e| e.params)
            .unwrap_or(&[])
    }

    /// Create a stabilizer algorithm instance from a type string and parameters.
    /// Returns `None` if the type_id is not found.
    pub fn create(&self, type_id: &str, params: &[ParamValue]) -> Option<Box<dyn StabilizerAlgorithm>> {
        self.entries.get(type_id).map(|reg| (reg.from_params)(params))
    }

    /// Create a stabilizer from a `StabilizerConfig`.
    /// Returns a pass-through if the config has no algorithm set.
    pub fn create_from_config(&self, config: &StabilizerConfig) -> Box<dyn StabilizerAlgorithm> {
        if config.algorithm.is_empty() || config.algorithm == "none" {
            return Box::new(PassThrough::new());
        }
        self.create(&config.algorithm, &config.params)
            .unwrap_or_else(|| {
                log::warn!("unknown stabilizer algorithm '{}', using pass-through", config.algorithm);
                Box::new(PassThrough::new())
            })
    }
}

/// Per-preset stabilizer configuration — stored in `BrushPreset`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StabilizerConfig {
    /// Algorithm type_id.  Empty string or "none" = pass-through.
    #[serde(default)]
    pub algorithm: String,
    /// Algorithm-specific parameter values.
    #[serde(default)]
    pub params: Vec<ParamValue>,
}

impl Default for StabilizerConfig {
    fn default() -> Self {
        Self {
            algorithm: String::new(),
            params: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_through_identity() {
        let mut stab = PassThrough::new();
        for i in 0..5 {
            let pt = PaintInformation {
                pos: [i as f32 * 10.0, 0.0],
                pressure: 0.5,
                ..Default::default()
            };
            let result = stab.push(pt);
            assert!(result.divergence_index.is_none());
        }
        assert_eq!(stab.len(), 5);
        assert!((stab.stabilized()[2].pos[0] - 20.0).abs() < 1e-6);
    }

    #[test]
    fn pass_through_clear() {
        let mut stab = PassThrough::new();
        stab.push(PaintInformation::default());
        assert_eq!(stab.len(), 1);
        stab.clear();
        assert_eq!(stab.len(), 0);
    }

    #[test]
    fn stabilizer_config_default_is_pass_through() {
        let config = StabilizerConfig::default();
        assert!(config.algorithm.is_empty());
        assert!(config.params.is_empty());
    }

    #[test]
    fn stabilizer_config_serde_round_trip() {
        let config = StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(0.6)],
        };
        let json = serde_json::to_string(&config).unwrap();
        let loaded: StabilizerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.algorithm, "laplacian");
        assert_eq!(loaded.params.len(), 1);
    }

    #[test]
    fn stabilizer_config_missing_fields_default() {
        let json = "{}";
        let config: StabilizerConfig = serde_json::from_str(json).unwrap();
        assert!(config.algorithm.is_empty());
        assert!(config.params.is_empty());
    }

    #[test]
    fn registry_creates_from_config() {
        let registry = StabilizerRegistry::new();

        // Empty config → pass-through.
        let config = StabilizerConfig::default();
        let stab = registry.create_from_config(&config);
        assert_eq!(stab.len(), 0);

        // "none" → pass-through.
        let config = StabilizerConfig { algorithm: "none".into(), params: vec![] };
        let stab = registry.create_from_config(&config);
        assert_eq!(stab.len(), 0);

        // Known algorithm.
        let config = StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(0.5)],
        };
        let mut stab = registry.create_from_config(&config);
        stab.push(PaintInformation::default());
        assert_eq!(stab.len(), 1);
    }

    #[test]
    fn registry_discovers_algorithms() {
        let registry = StabilizerRegistry::new();
        let types = registry.types();
        assert!(!types.is_empty(), "registry should discover at least one algorithm");
        assert!(types.iter().any(|(id, _, _)| *id == "laplacian"));
    }
}
