//! Contract tests for the modular config schema and built-in presets.
//!
//! These assert properties that must hold across every registered section
//! and preset so the schema stays internally consistent as new sections /
//! presets are added.

use std::collections::HashMap;

use darkly::config::{
    self,
    schema::{Pref, PrefDefault, PrefKind},
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
fn no_duplicate_pref_keys() {
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
fn preset_names_unique() {
    let names = config::preset_names();
    let mut seen = std::collections::HashSet::new();
    for name in &names {
        assert!(
            seen.insert(name.clone()),
            "duplicate preset name {:?}",
            name
        );
    }
    assert!(!names.is_empty(), "expected at least one built-in preset");
}

#[test]
fn preset_values_are_strings_for_action_keys() {
    // Every key in the preset's flattened output that's a hotkey or
    // mouseclick must be a Str (since both are stored as strings).
    for name in config::preset_names() {
        let values = config::preset_values(&name)
            .unwrap_or_else(|| panic!("preset {name:?} should be known"));
        for (key, value) in &values {
            if key.starts_with("hotkeys.") || key.starts_with("mouseclicks.") {
                assert!(
                    matches!(value, ConfigValue::Str(_)),
                    "preset {name:?} key {key:?}: expected Str, got {value:?}"
                );
            }
        }
    }
}

#[test]
fn empty_preset_clears_user_settings() {
    // The Krita preset declares no overrides; loading it produces an empty
    // values map, and applying that map to user_settings reverts every key
    // to its default.
    let values = config::preset_values("Krita").expect("Krita preset");
    assert!(
        values.is_empty(),
        "Krita preset should declare no overrides; got {} entries",
        values.len()
    );
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
