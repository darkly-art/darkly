//! Contract tests for the modular config schema.
//!
//! These assert properties that must hold across every registered section
//! so the schema stays internally consistent as sections are added.

use std::collections::HashMap;

use darkly::config::{
    self,
    schema::{Pref, PrefDefault, PrefKind, PresetValue},
    sections, ConfigValue,
};

fn all_prefs() -> Vec<&'static Pref> {
    sections::registrations()
        .iter()
        .flat_map(|s| s.prefs.iter())
        .collect()
}

// NOTE: a pref's `key` is a stable identifier independent of which section
// it currently lives in. Settings can be moved between sections (purely
// display groupings) without renaming the underlying key — there is
// intentionally no test enforcing key-prefix == section-id.

#[test]
fn no_duplicate_keys() {
    let mut seen: HashMap<&'static str, &'static str> = HashMap::new();
    for section in sections::registrations() {
        for pref in section.prefs {
            if let Some(prev_section) = seen.insert(pref.key, section.id) {
                panic!(
                    "duplicate pref key {:?}: declared in sections {:?} and {:?}",
                    pref.key, prev_section, section.id
                );
            }
        }
    }
}

#[test]
fn defaults_populated_and_kind_matches() {
    for pref in all_prefs() {
        let value = config::get(pref.key)
            .unwrap_or_else(|| panic!("default missing for pref key {:?}", pref.key));
        assert_kind_matches(pref.key, &pref.kind, &value);
        assert_default_matches(pref.key, &pref.kind, &pref.default);
    }
}

#[test]
fn preset_values_match_kind() {
    for pref in all_prefs() {
        for (preset_name, value) in pref.per_preset {
            assert_preset_value_kind(pref.key, preset_name, &pref.kind, value);
        }
    }
}

#[test]
fn preset_values_overlay_defaults() {
    // Every built-in template should be queryable. Its values for any key
    // are either an explicit per_preset override or the default.
    for name in config::preset_names() {
        let values = config::preset_values(&name)
            .unwrap_or_else(|| panic!("preset {:?} should be known", name));

        for pref in all_prefs() {
            let actual = values.get(pref.key).unwrap_or_else(|| {
                panic!("preset {:?} missing value for key {:?}", name, pref.key)
            });
            // Find the explicit override (if any) for this preset on this pref.
            let explicit = pref
                .per_preset
                .iter()
                .find(|(n, _)| *n == name.as_str())
                .map(|(_, v)| v);
            match explicit {
                Some(expected) => assert_preset_value_matches(pref.key, &name, expected, actual),
                None => {
                    // No override → default must show through.
                    let default = pref.default.to_config_value();
                    assert_eq!(
                        actual, &default,
                        "preset {:?} should fall back to default for key {:?}",
                        name, pref.key
                    );
                }
            }
        }
    }
}

#[test]
fn unknown_preset_returns_none() {
    assert!(config::preset_values("Bogus").is_none());
    assert!(config::preset_values("").is_none());
}

// --- helpers ---

fn assert_kind_matches(key: &str, kind: &PrefKind, value: &ConfigValue) {
    let ok = matches!(
        (kind, value),
        (PrefKind::Bool, ConfigValue::Bool(_))
            | (PrefKind::Int { .. }, ConfigValue::Int(_))
            | (PrefKind::Float { .. }, ConfigValue::Float(_))
            | (PrefKind::Str, ConfigValue::Str(_))
            | (PrefKind::Enum { .. }, ConfigValue::Str(_))
    );
    assert!(
        ok,
        "pref {:?}: kind/value mismatch (kind is {:?}, value is {:?})",
        key,
        kind_name(kind),
        value
    );
}

fn assert_default_matches(key: &str, kind: &PrefKind, default: &PrefDefault) {
    let ok = match (kind, default) {
        (PrefKind::Bool, PrefDefault::Bool(_)) => true,
        (PrefKind::Int { .. }, PrefDefault::Int(_)) => true,
        (PrefKind::Float { .. }, PrefDefault::Float(_)) => true,
        (PrefKind::Str, PrefDefault::Str(_)) => true,
        (PrefKind::Enum { options }, PrefDefault::Str(v)) => options.iter().any(|(k, _)| *k == *v),
        _ => false,
    };
    assert!(
        ok,
        "pref {:?}: default {:?} does not match kind {:?}",
        key,
        default_name(default),
        kind_name(kind)
    );
}

fn assert_preset_value_kind(key: &str, preset: &str, kind: &PrefKind, value: &PresetValue) {
    let ok = match (kind, value) {
        (PrefKind::Bool, PresetValue::Bool(_)) => true,
        (PrefKind::Int { .. }, PresetValue::Int(_)) => true,
        (PrefKind::Float { .. }, PresetValue::Float(_)) => true,
        (PrefKind::Str, PresetValue::Str(_)) => true,
        (PrefKind::Enum { options }, PresetValue::Str(v)) => options.iter().any(|(k, _)| *k == *v),
        _ => false,
    };
    assert!(
        ok,
        "pref {:?} preset {:?}: value {:?} does not match kind {:?}",
        key,
        preset,
        preset_value_name(value),
        kind_name(kind)
    );
}

fn assert_preset_value_matches(
    key: &str,
    preset: &str,
    expected: &PresetValue,
    actual: &ConfigValue,
) {
    let matches = match (expected, actual) {
        (PresetValue::Bool(a), ConfigValue::Bool(b)) => a == b,
        (PresetValue::Int(a), ConfigValue::Int(b)) => a == b,
        (PresetValue::Float(a), ConfigValue::Float(b)) => (a - b).abs() < f64::EPSILON,
        (PresetValue::Str(a), ConfigValue::Str(b)) => *a == b.as_str(),
        _ => false,
    };
    assert!(
        matches,
        "pref {:?} under preset {:?}: expected {:?}, got {:?}",
        key,
        preset,
        preset_value_name(expected),
        actual
    );
}

fn kind_name(kind: &PrefKind) -> &'static str {
    match kind {
        PrefKind::Bool => "bool",
        PrefKind::Int { .. } => "int",
        PrefKind::Float { .. } => "float",
        PrefKind::Str => "str",
        PrefKind::Enum { .. } => "enum",
    }
}

fn default_name(default: &PrefDefault) -> &'static str {
    match default {
        PrefDefault::Bool(_) => "Bool",
        PrefDefault::Int(_) => "Int",
        PrefDefault::Float(_) => "Float",
        PrefDefault::Str(_) => "Str",
    }
}

fn preset_value_name(value: &PresetValue) -> &'static str {
    match value {
        PresetValue::Bool(_) => "Bool",
        PresetValue::Int(_) => "Int",
        PresetValue::Float(_) => "Float",
        PresetValue::Str(_) => "Str",
    }
}
