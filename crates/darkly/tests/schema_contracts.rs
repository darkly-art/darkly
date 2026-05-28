//! Contract tests for the modular config schema.
//!
//! These assert properties that must hold across every registered section so
//! the schema stays internally consistent as new sections are added.

use std::collections::HashMap;

use darkly::config::{
    self,
    schema::{Pref, PrefKind},
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

/// Every schema-declared pref except `app.baseSettings` must have a value in
/// `defaults.yaml` or in every overlay — without that, `get_*` would panic
/// at runtime. `app.baseSettings` itself is the picker choice; it's
/// legitimately absent before the user picks an editor.
#[test]
fn every_pref_has_a_resolvable_value() {
    let names = config::base_names();
    assert!(!names.is_empty(), "expected at least one overlay");
    for overlay_name in &names {
        config::set("app.baseSettings", ConfigValue::Str(overlay_name.clone()));
        for pref in all_prefs() {
            if pref.key == "app.baseSettings" {
                continue;
            }
            let value = config::get(pref.key).unwrap_or_else(|| {
                panic!(
                    "pref {:?} has no value under overlay {:?} — \
                     missing from defaults.yaml and {:?}.yaml",
                    pref.key, overlay_name, overlay_name
                )
            });
            assert_kind_matches(pref.key, &pref.kind, &value);
        }
    }
    // Clean up so we don't leak state into other tests in the same process.
    config::reset("app.baseSettings");
}

#[test]
fn overlay_names_unique() {
    let names = config::base_names();
    let mut seen = std::collections::HashSet::new();
    for name in &names {
        assert!(seen.insert(name.clone()), "duplicate overlay name {name:?}");
    }
}

#[test]
fn app_base_settings_options_match_overlays() {
    let names = config::base_names();
    let from_const: Vec<String> = config::BASE_SETTINGS_OPTIONS
        .iter()
        .map(|(k, _)| (*k).to_string())
        .collect();
    assert_eq!(
        names, from_const,
        "BASE_SETTINGS_OPTIONS must match the discovered overlay list"
    );
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

fn kind_name(kind: &PrefKind) -> &'static str {
    match kind {
        PrefKind::Bool => "bool",
        PrefKind::Int { .. } => "int",
        PrefKind::Float { .. } => "float",
        PrefKind::Str => "str",
        PrefKind::Enum { .. } => "enum",
    }
}
