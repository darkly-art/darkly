//! Blend-mode registry — the single source of truth for "what blend modes exist."
//!
//! Each blend mode is one file under [`crate::gpu::blend_modes`] declaring
//! `type_id`, `display_name`, `category`, and `gpu_value`. The `gpu_value`
//! is the integer the WGSL composite shader switches on; nothing else in the
//! Rust process carries that integer as a parallel identity (no enum, no
//! `#[repr(u32)]` variants — the wire format and the in-memory representation
//! are both `&'static BlendModeRegistration`).
//!
//! Adding a blend mode = one new file + a matching arm in the shader's
//! dispatch. The registry is the authority; nothing in the engine ever
//! pattern-matches on individual blend modes.
//!
//! [`BlendProps`]: crate::layer::BlendProps

use std::collections::HashMap;
use std::sync::OnceLock;

/// Static metadata for one blend mode. Every layer/group holds a
/// `&'static BlendModeRegistration` directly; the GPU value is read straight
/// from `gpu_value`, no enum cast, no extra lookup.
///
/// The WGSL math is co-located via `wgsl_math` — adding a blend mode is one
/// new file, period. The composite shader's blend dispatch is assembled at
/// engine init from these fragments (see [`build_composite_source`]).
pub struct BlendModeRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// Visual grouping label for the UI dropdown ("Darken", "Lighten", etc.).
    pub category: &'static str,
    /// Integer the composite shader switches on. The shader's blend dispatch
    /// is the single consumer of this number; it is *not* used as identity
    /// anywhere in Rust — `type_id` is identity.
    pub gpu_value: u32,
    /// WGSL body for this blend mode's `case` arm. Receives `fg: vec4f` and
    /// `bg: vec4f` as straight-alpha colors, and must assign the blended
    /// straight-alpha RGB to the variable `Cs: vec3f`. May span multiple
    /// statements (use `\n`-separated WGSL); helpers declared above the
    /// `blend()` function in `shaders/composite.wgsl` are in scope.
    pub wgsl_math: &'static str,
}

pub struct BlendModeRegistry {
    /// Owned storage for every registered mode. Stable addresses while the
    /// registry lives (and it lives forever — see [`registry`]), so `&'static`
    /// references handed out by `get`/`default` stay valid forever.
    entries: Vec<BlendModeRegistration>,
    /// `type_id` → index into `entries`.
    by_type_id: HashMap<&'static str, usize>,
    /// Indices into `entries`, in GPU-value order — drives the dropdown so
    /// the UI lists modes in the conventional Photoshop / Krita ordering
    /// rather than alphabetic.
    ordered: Vec<usize>,
}

impl Default for BlendModeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BlendModeRegistry {
    pub fn new() -> Self {
        let entries: Vec<BlendModeRegistration> = super::blend_modes::registrations();
        let mut by_type_id = HashMap::with_capacity(entries.len());
        for (i, reg) in entries.iter().enumerate() {
            by_type_id.insert(reg.type_id, i);
        }
        let mut ordered: Vec<usize> = (0..entries.len()).collect();
        ordered.sort_by_key(|&i| entries[i].gpu_value);
        BlendModeRegistry {
            entries,
            by_type_id,
            ordered,
        }
    }

    /// Look up a registration by stable `type_id`. The returned reference
    /// is `&'static` because the registry itself is — callers can hold it
    /// indefinitely (and `BlendProps` does exactly that).
    pub fn get(&'static self, type_id: &str) -> Option<&'static BlendModeRegistration> {
        self.by_type_id.get(type_id).map(|&i| &self.entries[i])
    }

    /// The default blend mode (`"normal"`). Panics if `normal` is missing,
    /// which would mean the build system failed to discover its registration
    /// file — a build-time bug, not a runtime condition worth handling.
    pub fn default(&'static self) -> &'static BlendModeRegistration {
        self.get("normal")
            .expect("blend mode 'normal' must be registered")
    }

    /// All registered modes in GPU-value order. Used by the WASM bridge's
    /// `blend_mode_types()` query to populate the UI dropdown.
    pub fn all(&'static self) -> Vec<&'static BlendModeRegistration> {
        self.ordered.iter().map(|&i| &self.entries[i]).collect()
    }
}

/// Lazily-initialized process-wide blend-mode registry.
pub fn registry() -> &'static BlendModeRegistry {
    static REGISTRY: OnceLock<BlendModeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(BlendModeRegistry::new)
}

/// Assemble the composite shader's WGSL source by splicing each registered
/// blend mode's `wgsl_math` into the `// @blend-switch` marker in
/// `shaders/composite.wgsl`. Called once when the blend pipeline is built;
/// the output is one monolithic shader string fed to `create_shader_module`.
///
/// Per-mode arms are emitted in `gpu_value` order so the `case` numbers
/// match what the rest of the engine reads from `BlendModeRegistration.gpu_value`.
pub fn build_composite_source() -> String {
    const TEMPLATE: &str = include_str!("../../../../shaders/composite.wgsl");
    const MARKER: &str = "// @blend-switch";

    let mut arms = String::new();
    for reg in registry().all() {
        arms.push_str(&format!(
            "        case {}u: {{ {} }} // {}\n",
            reg.gpu_value, reg.wgsl_math, reg.display_name
        ));
    }
    arms.push_str("        default: { Cs = fg.rgb; }\n");

    let trimmed_arms = arms.trim_end_matches('\n');
    TEMPLATE.replacen(MARKER, trimmed_arms, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn normal_is_registered_and_is_the_default() {
        let r = registry();
        let normal = r.get("normal").expect("normal must be registered");
        assert_eq!(normal.type_id, "normal");
        assert!(std::ptr::eq(normal, r.default()));
    }

    #[test]
    fn registry_type_ids_and_gpu_values_are_unique() {
        let r = registry();
        let all = r.all();
        let ids: HashSet<&'static str> = all.iter().map(|reg| reg.type_id).collect();
        assert_eq!(ids.len(), all.len(), "duplicate type_id in registry");
        let values: HashSet<u32> = all.iter().map(|reg| reg.gpu_value).collect();
        assert_eq!(values.len(), all.len(), "duplicate gpu_value in registry");
    }

    #[test]
    fn ordered_by_gpu_value() {
        let r = registry();
        let all = r.all();
        for w in all.windows(2) {
            assert!(w[0].gpu_value < w[1].gpu_value);
        }
    }
}
