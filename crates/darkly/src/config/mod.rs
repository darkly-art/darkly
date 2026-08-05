pub mod chord;
pub mod schema;
pub mod sections;

#[allow(dead_code)]
mod presets_gen {
    include!(concat!(env!("OUT_DIR"), "/presets_gen.rs"));
}

pub use presets_gen::{BASE_SETTINGS_OPTIONS, DEFAULTS_YAML, OVERLAYS};

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

/// On-disk schema version for `user_settings.json`. Bump whenever a change
/// to the schema or YAML layers cannot be auto-cleaned by
/// [`super::schema`]-driven validation — e.g. a pref key is renamed, a
/// pref's kind changes shape (str→int, scalar→list), or the file's
/// envelope itself changes. Pre-release we just discard mismatched files
/// (per CLAUDE.md "No Migrations"); post-release this is the discriminator
/// migrations key off.
///
/// Forward-compatible changes don't need a bump: new prefs get default
/// values, removed pref keys are dropped by `validateOverrides`, and
/// numeric range changes are clamped.
pub const CONFIG_VERSION: u32 = 1;

/// A configuration value.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
}

/// Three-layer config store:
///
/// ```text
/// user override → overlay[active editor] → defaults (defaults.yaml)
/// ```
///
/// All three layers are sourced from YAML at startup: `defaults.yaml` is the
/// editor-AGNOSTIC baseline (always applied), each `<editor>.yaml` is one
/// equal-status overlay, and the user layer collects personal overrides.
///
/// The active editor is whatever `app.baseSettings` resolves to in the user
/// layer. The startup PresetPicker writes it before any consumer reads a
/// resolved value, so `get_*` getters can panic on a missing setting just
/// as before.
struct Config {
    defaults: HashMap<String, ConfigValue>,
    overlays: HashMap<String, HashMap<String, ConfigValue>>,
    user: HashMap<String, ConfigValue>,
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(Config::new());
}

impl Config {
    fn new() -> Self {
        let defaults = parse_yaml_preset(presets_gen::DEFAULTS_YAML)
            .unwrap_or_else(|e| panic!("failed to parse defaults.yaml: {e}"));
        let mut overlays = HashMap::new();
        for (name, yaml) in presets_gen::OVERLAYS {
            let map = parse_yaml_preset(yaml)
                .unwrap_or_else(|e| panic!("failed to parse overlay {name}: {e}"));
            overlays.insert((*name).to_string(), map);
        }
        Config {
            defaults,
            overlays,
            user: HashMap::new(),
        }
    }

    /// The one walk down the layer stack: named `overlay` above `defaults`,
    /// with the user layer consulted only when `include_user` is set.
    ///
    /// Every resolution goes through here so the layer order exists in exactly
    /// one place. `get` and `base_value` differ only in whether the user layer
    /// participates; the exporter differs only in naming the overlay outright
    /// rather than reading it from `app.baseSettings`.
    fn resolve(
        &self,
        overlay: Option<&str>,
        key: &str,
        include_user: bool,
    ) -> Option<&ConfigValue> {
        if include_user {
            if let Some(v) = self.user.get(key) {
                return Some(v);
            }
        }
        if let Some(name) = overlay {
            if let Some(v) = self.overlays.get(name).and_then(|m| m.get(key)) {
                return Some(v);
            }
        }
        self.defaults.get(key)
    }

    /// The overlay the user has selected, if any.
    fn active_overlay(&self) -> Option<&str> {
        match self.user.get("app.baseSettings") {
            Some(ConfigValue::Str(name)) => Some(name.as_str()),
            _ => None,
        }
    }

    /// Resolve a key down the layer stack.
    fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.resolve(self.active_overlay(), key, true)
    }

    /// What "Reset override on this key" would reveal — the layer below
    /// the user layer. Drives the Settings UI's "displayed default" and
    /// the Reset-affordance disabled state.
    fn base_value(&self, key: &str) -> Option<&ConfigValue> {
        self.resolve(self.active_overlay(), key, false)
    }
}

// ---------------------------------------------------------------------------
// YAML parsing — flattens the `{ hotkeys, mouse_clicks, settings }` shape
// into a dot-path key/value map, mirroring the legacy on-disk JSON model.
// ---------------------------------------------------------------------------

fn parse_yaml_preset(yaml: &str) -> Result<HashMap<String, ConfigValue>, String> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())?;
    let map = match value {
        serde_yaml_ng::Value::Mapping(m) => m,
        // Empty doc: zero entries (legitimately allowed for overlays that
        // only define a `name:` field and nothing else).
        serde_yaml_ng::Value::Null => return Ok(HashMap::new()),
        other => return Err(format!("expected top-level mapping, got {other:?}")),
    };

    let mut out: HashMap<String, ConfigValue> = HashMap::new();

    for (k, v) in map {
        let Some(key) = k.as_str() else { continue };
        match key {
            "name" | "description" => {
                // Metadata: not a config key.
                continue;
            }
            "hotkeys" => collect_string_facet(&v, "hotkeys.", &mut out)?,
            "mouse_clicks" => collect_string_facet(&v, "mouseclicks.", &mut out)?,
            "settings" => collect_settings_facet(&v, &mut out)?,
            _ => {
                // Tolerate top-level scalar entries by treating the key as a
                // settings key (handy for future hand-written YAML).
                if let Some(cv) = yaml_to_config_value(&v) {
                    out.insert(key.to_string(), cv);
                }
            }
        }
    }

    Ok(out)
}

/// `hotkeys` / `mouse_clicks` facets: keys map to either a single string
/// (one binding) or a list of strings (multiple alternative bindings).
/// Multi-binding entries are joined with a `|` separator — consumers know
/// to split on it. (Legacy: the only known multi-binding action is
/// `isolateLayer` with `[layerThumb:alt+click, maskThumb:alt+click]`.)
fn collect_string_facet(
    v: &serde_yaml_ng::Value,
    prefix: &str,
    out: &mut HashMap<String, ConfigValue>,
) -> Result<(), String> {
    let m = match v {
        serde_yaml_ng::Value::Mapping(m) => m,
        serde_yaml_ng::Value::Null => return Ok(()),
        other => return Err(format!("{prefix} expected a mapping, got {other:?}")),
    };
    for (k, v) in m {
        let Some(key) = k.as_str() else { continue };
        let full_key = format!("{prefix}{key}");
        match v {
            serde_yaml_ng::Value::String(s) => {
                out.insert(full_key, ConfigValue::Str(s.clone()));
            }
            serde_yaml_ng::Value::Sequence(seq) => {
                let mut parts: Vec<String> = Vec::with_capacity(seq.len());
                for item in seq {
                    if let serde_yaml_ng::Value::String(s) = item {
                        parts.push(s.clone());
                    } else {
                        return Err(format!("{full_key}: list item is not a string"));
                    }
                }
                out.insert(full_key, ConfigValue::Str(parts.join("|")));
            }
            serde_yaml_ng::Value::Null => {
                // Tolerate `key: ` with no value as "empty string" — a key
                // explicitly unbinds the action.
                out.insert(full_key, ConfigValue::Str(String::new()));
            }
            other => return Err(format!("{full_key}: unexpected value {other:?}")),
        }
    }
    Ok(())
}

/// `settings` facet: keys are already fully-qualified dot-paths; values are
/// bool/int/float/string.
fn collect_settings_facet(
    v: &serde_yaml_ng::Value,
    out: &mut HashMap<String, ConfigValue>,
) -> Result<(), String> {
    let m = match v {
        serde_yaml_ng::Value::Mapping(m) => m,
        serde_yaml_ng::Value::Null => return Ok(()),
        other => return Err(format!("settings expected a mapping, got {other:?}")),
    };
    for (k, v) in m {
        let Some(key) = k.as_str() else { continue };
        if let Some(cv) = yaml_to_config_value(v) {
            out.insert(key.to_string(), cv);
        } else {
            return Err(format!("settings.{key}: unsupported value {v:?}"));
        }
    }
    Ok(())
}

fn yaml_to_config_value(v: &serde_yaml_ng::Value) -> Option<ConfigValue> {
    match v {
        serde_yaml_ng::Value::Bool(b) => Some(ConfigValue::Bool(*b)),
        serde_yaml_ng::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(ConfigValue::Int(i))
            } else {
                n.as_f64().map(ConfigValue::Float)
            }
        }
        serde_yaml_ng::Value::String(s) => Some(ConfigValue::Str(s.clone())),
        serde_yaml_ng::Value::Null => None,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public module-level API (delegates to thread-local)
// ---------------------------------------------------------------------------

/// Get a config value by dot-path key. Returns `None` only if the key is
/// absent from every layer.
pub fn get(key: &str) -> Option<ConfigValue> {
    CONFIG.with(|c| c.borrow().get(key).cloned())
}

/// Get a float value. Coerces Int → f64. Panics if the key is missing.
pub fn get_f64(key: &str) -> f64 {
    match get(key) {
        Some(ConfigValue::Float(f)) => f,
        Some(ConfigValue::Int(i)) => i as f64,
        other => panic!("config key {key:?}: expected numeric, got {other:?}"),
    }
}

/// Get an integer value. Panics if the key is missing or wrong type.
pub fn get_i64(key: &str) -> i64 {
    match get(key) {
        Some(ConfigValue::Int(i)) => i,
        other => panic!("config key {key:?}: expected int, got {other:?}"),
    }
}

/// Get a string value. Panics if the key is missing or wrong type.
pub fn get_str(key: &str) -> String {
    match get(key) {
        Some(ConfigValue::Str(s)) => s,
        other => panic!("config key {key:?}: expected string, got {other:?}"),
    }
}

/// Get a boolean value. Panics if the key is missing or wrong type.
pub fn get_bool(key: &str) -> bool {
    match get(key) {
        Some(ConfigValue::Bool(b)) => b,
        other => panic!("config key {key:?}: expected bool, got {other:?}"),
    }
}

/// Layer-below-user value for a key (overlay → defaults). Drives "Reset"
/// affordances and the Settings UI's displayed-default text.
pub fn base_value(key: &str) -> Option<ConfigValue> {
    CONFIG.with(|c| c.borrow().base_value(key).cloned())
}

/// Set a value in the user layer.
pub fn set(key: &str, value: ConfigValue) {
    CONFIG.with(|c| {
        c.borrow_mut().user.insert(key.to_string(), value);
    });
}

/// Remove a user override for a key. Reveals the overlay/default below.
pub fn reset(key: &str) {
    CONFIG.with(|c| {
        c.borrow_mut().user.remove(key);
    });
}

/// Clear every user override **except** `app.baseSettings` — the picker
/// choice survives a global reset so the user isn't bumped back to the
/// first-run picker by clicking "Reset everything".
pub fn reset_all() {
    CONFIG.with(|c| {
        let mut cfg = c.borrow_mut();
        let base = cfg.user.remove("app.baseSettings");
        cfg.user.clear();
        if let Some(v) = base {
            cfg.user.insert("app.baseSettings".to_string(), v);
        }
    });
}

/// Equal-status overlay display names (alphabetical order).
pub fn base_names() -> Vec<String> {
    presets_gen::OVERLAYS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// True if the declared `PrefKind` for `key` is `Int` (used by the WASM
/// bridge to disambiguate JS numbers when serializing back to Rust).
pub fn kind_is_int(key: &str) -> bool {
    for section in sections::registrations() {
        for pref in section.prefs {
            if pref.key == key {
                return matches!(pref.kind, schema::PrefKind::Int { .. });
            }
        }
    }
    false
}

/// Return the full schema as a serializable view. Used by the WASM bridge to
/// feed the Settings UI.
/// The editor-agnostic baseline value for a key — the bottom layer alone, with
/// no user override and no editor overlay.
///
/// Reads `defaults.yaml` directly rather than the process-global config, so a
/// caller that only wants to describe the schema (the metadata exporter, the
/// settings projection) needs no initialization and cannot be perturbed by
/// whatever the running editor has chosen.
/// One effective binding: the raw preset string, its parsed prefix, and the
/// chord rendered for both platform conventions.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    /// The binding exactly as the preset YAML declares it.
    pub raw: String,
    /// Prefix parts, each absent unless the binding declares it.
    pub site: Option<String>,
    pub scope: Option<String>,
    pub brush: Option<String>,
    /// The chord rendered with Apple modifier glyphs.
    pub mac: String,
    /// The chord rendered with the Windows/Linux modifier names.
    pub other: String,
}

impl Binding {
    fn parse(raw: &str) -> Self {
        let p = chord::parse_binding(raw);
        Binding {
            raw: raw.to_string(),
            site: p.site,
            scope: p.scope,
            brush: p.brush,
            mac: chord::format_chord(&p.chord, chord::Platform::Mac),
            other: chord::format_chord(&p.chord, chord::Platform::Other),
        }
    }
}

/// Every hotkey and mouse binding a named preset resolves to, with no user
/// layer. `None` resolves the editor-agnostic baseline alone.
///
/// Keys are action ids; every value holds at least one [`Binding`]. An action
/// the preset binds nothing to is **absent** — the map is already resolved, so
/// absence is a complete statement rather than an instruction to look in a
/// lower layer. No empty vector is ever emitted.
///
/// Builds from the generated presets directly rather than the process-global
/// config, so a caller needs no initialization and cannot be perturbed by
/// whatever the running editor has selected.
pub fn preset_bindings(overlay: Option<&str>) -> BTreeMap<String, Vec<Binding>> {
    let defaults = parse_yaml_preset(presets_gen::DEFAULTS_YAML)
        .unwrap_or_else(|e| panic!("failed to parse defaults.yaml: {e}"));
    let mut overlays = HashMap::new();
    for (name, yaml) in presets_gen::OVERLAYS {
        let map = parse_yaml_preset(yaml)
            .unwrap_or_else(|e| panic!("failed to parse overlay {name}: {e}"));
        overlays.insert((*name).to_string(), map);
    }
    let config = Config {
        defaults,
        overlays,
        user: HashMap::new(),
    };

    // The union of keys the agnostic layer and this overlay declare — every key
    // that could resolve to anything under this preset. Deduped: a key both
    // layers declare is one key that resolves once, not two.
    let mut keys: Vec<&String> = config.defaults.keys().collect();
    if let Some(name) = overlay {
        if let Some(m) = config.overlays.get(name) {
            keys.extend(m.keys());
        }
    }
    keys.sort();
    keys.dedup();

    let mut out: BTreeMap<String, Vec<Binding>> = BTreeMap::new();
    for key in keys {
        let Some(action) = key
            .strip_prefix("hotkeys.")
            .or_else(|| key.strip_prefix("mouseclicks."))
        else {
            continue;
        };
        let Some(ConfigValue::Str(raw)) = config.resolve(overlay, key, false) else {
            continue;
        };
        // `collect_string_facet` joins a YAML list with `|`, so one id can
        // carry several chords. Shipping the joined string would push a
        // splitting rule onto every consumer.
        let bindings: Vec<Binding> = raw
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Binding::parse)
            .collect();
        if bindings.is_empty() {
            continue;
        }
        out.entry(action.to_string()).or_default().extend(bindings);
    }
    out
}

pub fn agnostic_default(key: &str) -> Option<ConfigValue> {
    thread_local! {
        static DEFAULTS: HashMap<String, ConfigValue> =
            parse_yaml_preset(presets_gen::DEFAULTS_YAML)
                .unwrap_or_else(|e| panic!("failed to parse defaults.yaml: {e}"));
    }
    DEFAULTS.with(|d| d.get(key).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_state() {
        CONFIG.with(|c| {
            c.borrow_mut().user.clear();
        });
    }

    fn pick(name: &str) {
        set("app.baseSettings", ConfigValue::Str(name.to_string()));
    }

    #[test]
    fn defaults_from_yaml() {
        reset_state();
        // Agnostic defaults present without picking an editor.
        assert_eq!(get_i64("animation.veil_divisor"), 2);
        assert_eq!(get_i64("canvas.width"), 1920);
        // Autosave section (schema in sections/autosave.rs, defaults in yaml).
        assert!(get_bool("autosave.enabled"));
        assert_eq!(get_i64("autosave.intervalSeconds"), 120);
        assert!(kind_is_int("autosave.intervalSeconds"));
        assert_eq!(get_str("hotkeys.nav.trigger"), "Space");
        assert!(get_bool("input.fingerPainting"));
        // Darkly-original hotkey defined in defaults.yaml (no reference-editor
        // prior art, so it stays in the agnostic baseline).
        assert_eq!(get_str("hotkeys.addBrushNode"), "Shift+KeyA");
    }

    #[test]
    fn overlay_resolves_above_defaults() {
        reset_state();
        pick("Krita");
        // Krita-specific override (defined only in krita.yaml).
        assert_eq!(get_str("hotkeys.brushTool"), "KeyB");
        // Defaults still show through where the overlay is silent —
        // addBrushNode is Darkly-original and no overlay defines it.
        assert_eq!(get_str("hotkeys.addBrushNode"), "Shift+KeyA");

        // Switching to Photoshop swaps the overlay live.
        pick("Photoshop");
        assert_eq!(get_str("hotkeys.rectSelectTool"), "KeyM");
        assert_eq!(get_str("hotkeys.addBrushNode"), "Shift+KeyA");
    }

    #[test]
    fn user_wins_over_overlay_and_defaults() {
        reset_state();
        pick("Krita");
        set("hotkeys.brushTool", ConfigValue::Str("KeyZ".into()));
        assert_eq!(get_str("hotkeys.brushTool"), "KeyZ");
        reset("hotkeys.brushTool");
        // Falls back to overlay value, not defaults.
        assert_eq!(get_str("hotkeys.brushTool"), "KeyB");
    }

    #[test]
    fn reset_all_preserves_base_choice() {
        reset_state();
        pick("Photoshop");
        set("hotkeys.brushTool", ConfigValue::Str("KeyZ".into()));
        reset_all();
        // Override is gone…
        assert_eq!(get_str("hotkeys.brushTool"), "KeyB");
        // …but the base choice survives.
        assert_eq!(get_str("app.baseSettings"), "Photoshop");
    }

    #[test]
    fn base_value_skips_user_layer() {
        reset_state();
        pick("Krita");
        set("hotkeys.brushTool", ConfigValue::Str("KeyZ".into()));
        // `base_value` is what a Reset would reveal — the overlay value.
        match base_value("hotkeys.brushTool") {
            Some(ConfigValue::Str(s)) => assert_eq!(s, "KeyB"),
            other => panic!("expected overlay value, got {other:?}"),
        }
    }

    #[test]
    fn base_names_lists_overlays_alphabetically() {
        let names = base_names();
        assert!(!names.is_empty(), "expected at least one overlay");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn kind_is_int_uses_schema() {
        // `canvas.width` is an int pref → true.
        assert!(kind_is_int("canvas.width"));
        // `nav.panSensitivity` is a float pref → false.
        assert!(!kind_is_int("nav.panSensitivity"));
        // Unknown key → false (defensive).
        assert!(!kind_is_int("bogus.key"));
    }

    /// Resolution never returns a short map: every id the agnostic layer or
    /// this overlay declares comes back, because both layers are walked. An
    /// inheritance bug shows up here as a missing key.
    #[test]
    fn preset_bindings_covers_every_key_in_every_preset() {
        let binding_ids = |yaml: &str| -> Vec<String> {
            parse_yaml_preset(yaml)
                .unwrap()
                .into_iter()
                .filter_map(|(k, v)| {
                    // A key whose value is an empty string binds nothing.
                    match v {
                        ConfigValue::Str(s) if !s.trim().is_empty() => Some(k),
                        _ => None,
                    }
                })
                .filter_map(|k| {
                    k.strip_prefix("hotkeys.")
                        .or_else(|| k.strip_prefix("mouseclicks."))
                        .map(str::to_string)
                })
                .collect()
        };

        for (name, yaml) in presets_gen::OVERLAYS {
            let mut expected: Vec<String> = binding_ids(presets_gen::DEFAULTS_YAML);
            expected.extend(binding_ids(yaml));
            expected.sort();
            expected.dedup();

            let got = preset_bindings(Some(name));
            let mut got_ids: Vec<String> = got.keys().cloned().collect();
            got_ids.sort();
            assert_eq!(
                got_ids, expected,
                "preset `{name}` resolves a different id set than its layers declare"
            );
        }

        let mut expected = binding_ids(presets_gen::DEFAULTS_YAML);
        expected.sort();
        expected.dedup();
        let mut got_ids: Vec<String> = preset_bindings(None).keys().cloned().collect();
        got_ids.sort();
        assert_eq!(
            got_ids, expected,
            "the agnostic baseline resolves a different id set"
        );
    }

    /// An overlay sits *above* the baseline rather than replacing it: every id
    /// the agnostic layer binds is still bound under every overlay, with the
    /// overlay's chords where it overrides and the baseline's otherwise.
    #[test]
    fn preset_bindings_inherits_the_agnostic_layer() {
        let base = preset_bindings(None);
        for (name, yaml) in presets_gen::OVERLAYS {
            let overlay_map = parse_yaml_preset(yaml).unwrap();
            let resolved = preset_bindings(Some(name));
            for (id, base_bindings) in &base {
                let got = resolved.get(id).unwrap_or_else(|| {
                    panic!("preset `{name}` dropped `{id}`, which the baseline binds")
                });
                let overridden = overlay_map.contains_key(&format!("hotkeys.{id}"))
                    || overlay_map.contains_key(&format!("mouseclicks.{id}"));
                if !overridden {
                    let a: Vec<&String> = base_bindings.iter().map(|b| &b.raw).collect();
                    let b: Vec<&String> = got.iter().map(|b| &b.raw).collect();
                    assert_eq!(a, b, "preset `{name}` changed `{id}` without overriding it");
                }
            }
        }
    }

    /// Absence means "binds nothing"; there is no empty vector to misread as
    /// "explicitly unbound". No preset mechanism produces one today, and this
    /// keeps the two-valued design from creeping back without one.
    #[test]
    fn preset_bindings_never_emits_an_empty_vec() {
        let mut presets: Vec<Option<&str>> = vec![None];
        presets.extend(presets_gen::OVERLAYS.iter().map(|(n, _)| Some(*n)));
        for preset in presets {
            for (id, bindings) in preset_bindings(preset) {
                assert!(
                    !bindings.is_empty(),
                    "preset {preset:?} emitted an empty binding list for `{id}`"
                );
                for b in &bindings {
                    assert!(!b.raw.is_empty(), "`{id}` carries an empty raw binding");
                    assert!(!b.mac.is_empty(), "`{id}` renders empty on mac");
                    assert!(!b.other.is_empty(), "`{id}` renders empty off mac");
                }
            }
        }
    }

    /// The new public resolution API and `Config::get` must not drift: with no
    /// user layer they are the same walk, and this pins that they agree.
    #[test]
    fn preset_bindings_matches_config_get_with_no_user_layer() {
        for (name, _) in presets_gen::OVERLAYS {
            reset_state();
            pick(name);
            for (id, bindings) in preset_bindings(Some(name)) {
                let got: Vec<String> = bindings.iter().map(|b| b.raw.clone()).collect();

                // An action can be bound in both facets — Krita gives
                // `isolateLayer` a key *and* two mouse chords — and the
                // exporter merges them, key-sorted, under the one id. Rebuild
                // that from `Config::get` and require the same list.
                let mut expected: Vec<String> = Vec::new();
                for facet in ["hotkeys", "mouseclicks"] {
                    let raw = CONFIG.with(|c| c.borrow().get(&format!("{facet}.{id}")).cloned());
                    if let Some(ConfigValue::Str(raw)) = raw {
                        expected.extend(
                            raw.split('|')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        );
                    }
                }
                assert!(
                    !expected.is_empty(),
                    "`{id}` resolves through preset_bindings but not through Config::get"
                );
                assert_eq!(
                    got, expected,
                    "`{id}` differs between the two paths under `{name}`"
                );
            }
        }
        reset_state();
    }

    /// Prints the measured id counts per preset. Not an assertion — the counts
    /// move whenever a preset gains a binding, which is why the tests above
    /// assert the *rule* that produces them instead.
    #[test]
    #[ignore = "reporting only"]
    fn report_binding_counts() {
        let mut union: std::collections::BTreeSet<String> = Default::default();
        let base = preset_bindings(None);
        union.extend(base.keys().cloned());
        println!("defaults: {}", base.len());
        for (name, _) in presets_gen::OVERLAYS {
            let m = preset_bindings(Some(name));
            union.extend(m.keys().cloned());
            println!("{name}: {}", m.len());
        }
        println!("union: {}", union.len());
    }

    /// Every tool-selecting binding in every preset names a tool that actually
    /// registers it. A preset naming an id no registration declares is a key
    /// that silently does nothing — the bug class that once shipped a dead
    /// Ctrl+I. The ids are deliberately not derivable from `type_id`
    /// (`colorpicker` declares `colorPickerTool`), so nothing but this
    /// comparison catches a typo on either side.
    #[test]
    fn every_preset_binding_names_a_registered_target() {
        let registered: Vec<&'static str> = crate::tool::registry()
            .types()
            .into_iter()
            .map(|reg| reg.hotkey_action)
            .collect();

        let mut presets: Vec<(&str, &str)> = vec![("defaults", presets_gen::DEFAULTS_YAML)];
        presets.extend(presets_gen::OVERLAYS.iter().copied());

        let mut checked = 0usize;
        for (preset, yaml) in presets {
            let map = parse_yaml_preset(yaml)
                .unwrap_or_else(|e| panic!("failed to parse preset {preset}: {e}"));
            for key in map.keys() {
                let Some(action) = key
                    .strip_prefix("hotkeys.")
                    .or_else(|| key.strip_prefix("mouseclicks."))
                else {
                    continue;
                };
                // Tool-selecting actions are the ones this registry owns; the
                // rest of the binding surface belongs to the TypeScript action
                // registry and is out of scope here.
                if !action.ends_with("Tool") {
                    continue;
                }
                assert!(
                    registered.contains(&action),
                    "preset `{preset}` binds `{action}`, which no tool registers \
                     (registered: {registered:?})"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no tool bindings found in any preset — the scan is looking in the wrong place"
        );
    }
}
