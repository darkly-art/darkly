//! Schema metadata for darkly's preferences.
//!
//! Every preference lives in a section. Sections are auto-discovered from
//! `config/sections/*.rs` via `build.rs` — the same pattern as veils, tools,
//! and brush nodes. Runtime storage is the three-layer [`super::Config`]
//! (user → overlay → defaults). The schema declares only *type/range/widget*
//! — it does not own values. Defaults come from the bundled YAML presets in
//! `crates/darkly/presets/`.
//!
//! Storage type vs. widget: [`PrefKind`] describes what's stored;
//! [`WidgetHint`] describes how the Settings modal renders it. They're
//! orthogonal — `Bool` is always a toggle, but a `Str` might render as plain
//! text, a hotkey capture, a mouse-binding capture, or a color picker.

/// A logical grouping of related preferences — purely a display affordance.
/// Sections may be reorganized without renaming any pref keys; a key's
/// section is metadata, not part of its identity.
pub struct SchemaSection {
    /// Stable identifier for the section itself (used by the Settings UI to
    /// remember which tab was active, etc.).
    pub id: &'static str,
    /// Human-readable label shown in the tab list.
    pub display_name: &'static str,
    /// Optional one-line help shown above the section's prefs.
    pub description: Option<&'static str>,
    /// Optional FontAwesome class for the tab icon (e.g. `"fa-solid fa-palette"`).
    pub icon: Option<&'static str>,
    /// Sort key for the tab list. Lower = earlier. Ties broken by `id`.
    pub order: i32,
    /// Preferences owned by this section.
    pub prefs: &'static [Pref],
}

/// One declared preference.
pub struct Pref {
    /// Stable, globally-unique dot-path key (e.g. `"canvas.width"`).
    /// Independent of which section the pref currently lives in.
    pub key: &'static str,
    /// Label rendered in the Settings UI.
    pub display_name: &'static str,
    /// Optional longer help text (tooltip / inline explanation).
    pub description: Option<&'static str>,
    /// Storage shape + range/option metadata.
    pub kind: PrefKind,
    /// Hint for which widget the Settings UI should render.
    pub widget: WidgetHint,
}

/// What kind of value a pref stores and what constraints it has.
pub enum PrefKind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
    /// Free-form string. Pair with a [`WidgetHint`] for specialized inputs.
    Str,
    /// One-of-N. Stored as `Str` (the `value` key); `options[i].0` is the
    /// machine value, `options[i].1` is the human label.
    Enum {
        options: &'static [(&'static str, &'static str)],
    },
}

/// How the Settings UI should render a pref. Orthogonal to [`PrefKind`] so new
/// specialized inputs (color picker, font picker, …) can be added without
/// touching the storage model.
pub enum WidgetHint {
    /// Pick by kind: Bool→toggle, numeric→slider, Str→text input, Enum→dropdown.
    Auto,
    /// Numeric rendered as a plain number input (no slider). Use for values
    /// where dragging is impractical (canvas dimensions, wide-range counts).
    NumberInput,
    /// `Str` rendered as a tinykeys-style hotkey capture box.
    Hotkey,
    /// `Str` rendered as a color picker (hex `#rrggbb`).
    Color,
    /// Persisted via the backend but not rendered in the Settings UI.
    /// For UI state (panel visibility, panel sizes, recent-files list, …)
    /// that lives on the same persistence pipe as user-visible prefs but
    /// shouldn't show up as a "setting".
    Hidden,
}

// ---------------------------------------------------------------------------
// Flat serialization views for the WASM bridge.
// ---------------------------------------------------------------------------

/// Flat serialization of a [`SchemaSection`] with prefs already projected.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<&'static str>,
    pub order: i32,
    pub prefs: Vec<PrefInfo>,
}

/// Flat view of a single [`Pref`] with kind/range/options inlined.
/// Avoids a tagged enum so the frontend can consume the JSON without
/// discriminator unwrapping. No `default` field — defaults live in
/// the YAML overlay/agnostic layers, not in the schema.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefInfo {
    pub key: &'static str,
    pub display_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    /// `"bool" | "int" | "float" | "str" | "enum"`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Populated for `"enum"` kinds only: `[[value, label], ...]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    /// `"auto" | "numberInput" | "hotkey" | "mouseBinding" | "color"`.
    pub widget: &'static str,
}

impl SectionInfo {
    pub fn from_section(section: &SchemaSection) -> Self {
        SectionInfo {
            id: section.id,
            display_name: section.display_name,
            description: section.description,
            icon: section.icon,
            order: section.order,
            prefs: section.prefs.iter().map(PrefInfo::from_pref).collect(),
        }
    }
}

impl PrefInfo {
    pub fn from_pref(pref: &Pref) -> Self {
        let (kind, min, max, options) = match &pref.kind {
            PrefKind::Bool => ("bool", None, None, None),
            PrefKind::Int { min, max } => ("int", Some(*min as f64), Some(*max as f64), None),
            PrefKind::Float { min, max } => ("float", Some(*min), Some(*max), None),
            PrefKind::Str => ("str", None, None, None),
            PrefKind::Enum { options } => ("enum", None, None, Some(serde_json::json!(options))),
        };
        PrefInfo {
            key: pref.key,
            display_name: pref.display_name,
            description: pref.description,
            kind,
            min,
            max,
            options,
            widget: widget_hint_str(&pref.widget),
        }
    }
}

fn widget_hint_str(hint: &WidgetHint) -> &'static str {
    match hint {
        WidgetHint::Auto => "auto",
        WidgetHint::NumberInput => "numberInput",
        WidgetHint::Hotkey => "hotkey",
        WidgetHint::Color => "color",
        WidgetHint::Hidden => "hidden",
    }
}
