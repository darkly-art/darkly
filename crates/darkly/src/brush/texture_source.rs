//! How a compiled brush's `@group(3)` texture slots resolve to bound GPU
//! textures at pipeline-build time.
//!
//! A brush's fragment shader samples zero or more `@group(3)` textures via
//! [`crate::brush::wgsl::sample_graph_texture`]. Two kinds of node produce
//! those slots, and they differ only in *how the texture is obtained*:
//!
//! - [`ResolvedSource::Named`] — a texture already in the
//!   [`crate::gpu::texture_registry::TextureRegistry`], looked up by name
//!   (the `image` node).
//! - [`ResolvedSource::Baked`] — a procedural tile baked once by running the
//!   existing fBm shader into a texture and cached by its field-defining
//!   parameters (the `noise` node, when its field is static). Baking turns an
//!   ~80-hash per-fragment fBm kernel — re-run per canvas pixel per
//!   overlapping dab — into a single `textureSample`.
//! - [`ResolvedSource::Live`] — a texture the requesting node republishes
//!   every flush (`clone_source`'s stroke snapshot, `pickup`'s per-dab
//!   atlas). Resolved at bind time from the live table, so the slot
//!   survives the texture being reallocated mid-stroke, and falls back to
//!   `_fallback` when nothing has been published — which is what makes the
//!   cursor preview neutral without a special case.
//!
//! All three converge on the identical emission; the only divergence is a
//! three-arm match at the single bind point (`make_bind_group`). This is data
//! only — no trait, no registry. When a *third* bakeable field lands (e.g. a
//! procedural paper/hatch source), promote [`BakeKind`] to a `Bakeable` trait
//! with per-variant files, mirroring `gpu/veils/*`; the arms here are one
//! expression each and are not yet a subsystem.

/// How a `@group(3)` slot resolves to a bound texture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResolvedSource {
    /// A texture already in the registry, by name (the `image` node).
    Named(String),
    /// A procedural tile to bake-or-reuse, keyed by field-defining
    /// parameters (the `noise` node, static-field path).
    Baked(BakeSpec),
    /// A texture the requesting node republishes every flush. Resolved at
    /// bind time from the live table rather than at pipeline-build time,
    /// so the slot survives the texture being reallocated mid-stroke.
    Live(LiveSource),
}

impl ResolvedSource {
    /// A short human-readable label for this source, for diagnostics /
    /// error messages that enumerate a brush's texture slots.
    pub fn binding_label(&self) -> String {
        match self {
            ResolvedSource::Named(name) => name.clone(),
            ResolvedSource::Baked(spec) => format!("<baked {}>", spec.kind.label()),
            ResolvedSource::Live(live) => format!("<live {}>", live.label()),
        }
    }

    /// Whether this slot is republished per flush. A brush with any live
    /// slot cannot cache its `@group(3)` bind group on the pipeline.
    pub fn is_live(&self) -> bool {
        matches!(self, ResolvedSource::Live(_))
    }
}

/// A `@group(3)` texture supplied fresh once per flush by the node that
/// requested it, rather than resolved against the registry or the bake
/// cache at pipeline-build time.
///
/// Each producer publishes its view through
/// [`crate::brush::gpu_context::BrushGpuContext::publish_live_texture`]
/// during its own `flush_dabs`, which the runner dispatches in topological
/// order — so a producer upstream of the terminal has always published by
/// the time the terminal binds. A slot with nothing published falls back to
/// the registry's `_fallback` tile, which is what makes the cursor preview
/// (no stroke, no dabs, nothing published) render neutrally with no
/// special-casing in the preview pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiveSource {
    /// The stroke's frozen source snapshot — the cross-layer / merged
    /// snapshot when one was captured, else the pre-stroke snapshot
    /// (same-layer clone). Published by `clone_source`.
    StrokeSnapshot,
    /// The per-dab pickup atlas: one texel per dab holding the
    /// neighbourhood average of the dry canvas under it. Published by
    /// `pickup`, which renders it in its own `flush_dabs`.
    PickupAtlas,
}

impl LiveSource {
    fn label(&self) -> &'static str {
        match self {
            LiveSource::StrokeSnapshot => "stroke snapshot",
            LiveSource::PickupAtlas => "pickup atlas",
        }
    }
}

/// The channel layout a bake produces: R8 grayscale for the `noise` node's
/// scalar `value` output, RGBA8 for its chromatic `color` output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BakeChannels {
    Grayscale,
    Rgba,
}

/// A fully-specified, hashable recipe for one baked procedural tile.
///
/// Holds **field-defining** parameters only — the things that change the tile
/// *content*. Sample-time parameters (`scale`, `variation`, `rotation`) are
/// deliberately absent: they are applied by the sampling frame at read time
/// (they move *where* the tile is sampled, not *what it contains*), so one
/// baked tile serves every scale/variation/rotation and both Canvas and Dab
/// space. Two brushes with an equal `BakeSpec` share one cached tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BakeSpec {
    /// Which procedural field to bake (today: only [`BakeKind::Noise`]).
    pub kind: BakeKind,
    /// R8 vs RGBA8 (from the `noise` node's `value` vs `color` output).
    pub channels: BakeChannels,
    /// Square tile edge in texels. Derived from the field's octave count so
    /// the tile resolves its finest frequency (see the noise node).
    pub resolution: u32,
}

impl BakeSpec {
    /// How many field units the baked tile spans on each axis — equivalently,
    /// the field's **repeat period**: sampled through the Repeat sampler the
    /// tile wraps once per `FIELD_SPAN` field units. Made large so the field
    /// does not visibly repeat within a normal view. The bake shader maps a
    /// texel `uv ∈ [0,1)` to `uv * FIELD_SPAN` before calling the fBm.
    ///
    /// Independent of [`resolution_for_octaves`](Self::resolution_for_octaves):
    /// the period sets how far the fixed texel budget is stretched across the
    /// plane, not how many texels the tile holds. Enlarging it costs no memory
    /// — it softens fine detail instead (the texels cover more field units).
    pub const FIELD_SPAN: f32 = 128.0;

    /// Reference detail window (field units) the tile is sized to resolve at
    /// Nyquist — the detail axis, held **separate** from [`FIELD_SPAN`] (the
    /// period axis) so neither constant is overloaded. The tile holds enough
    /// texels to resolve the finest octave across *this* window; the larger
    /// real [`FIELD_SPAN`] stretches those texels further, so fine octaves
    /// soften rather than the tile growing. Raise this toward `FIELD_SPAN`
    /// (and the memory clamp) to trade memory for sharpness across the span.
    const DETAIL_SPAN: u32 = 16;

    /// Tile edge resolution (texels) that resolves the finest fBm frequency
    /// for `octaves` across [`DETAIL_SPAN`], clamped to a sane memory band.
    /// With a base cell of 1 field unit and octaves doubling frequency, the
    /// finest feature is `DETAIL_SPAN / 2^(octaves-1)` field units; at ~2
    /// texels per finest half-feature that is `DETAIL_SPAN * 2^(octaves-1) *
    /// 2` texels. Clamped to `[512, 2048]` (1–16 MiB RGBA8; ¼ that for R8),
    /// trading fine detail at high octaves for bounded memory. Deliberately
    /// does not scale with [`FIELD_SPAN`] — see there.
    pub fn resolution_for_octaves(octaves: i32) -> u32 {
        let finest = 1u32 << (octaves.clamp(1, 8) - 1) as u32;
        (Self::DETAIL_SPAN * finest * 2).clamp(512, 2048)
    }
}

/// The procedural field a [`ResolvedSource::Baked`] tile bakes.
///
/// A single-variant enum today. A future procedural paper/hatch/gradient
/// source adds a variant here and a match arm in the bake function —
/// additive, no consumer edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BakeKind {
    /// Domain-warped, per-octave-rotated fBm — the `noise` node's field.
    ///
    /// `warp`/`roughness` are `f32` at the node but must key a `Hash + Eq`
    /// spec, so they are quantized (see [`BakeKind::quantize`]) before
    /// entering here. A bake is a visual approximation, so sub-quantum
    /// parameter deltas sharing a tile is correct, not a bug. `octaves`,
    /// `warp_q`, `roughness_q`, and `seed` here are the **already-clamped**
    /// values the node's live path would use — the bake must not re-clamp.
    Noise {
        seed: u32,
        octaves: i32,
        warp_q: u32,
        roughness_q: u32,
    },
}

impl BakeKind {
    /// Quantum for turning an `f32` parameter into a hashable `u32` key.
    /// 1e-4 resolution — finer than any visible difference in a baked tile.
    pub const QUANTUM: f32 = 1.0e4;

    /// Quantize an `f32` field parameter (assumed already clamped to its
    /// valid range) into the `u32` key stored in [`BakeKind::Noise`].
    pub fn quantize(v: f32) -> u32 {
        (v * Self::QUANTUM).round().max(0.0) as u32
    }

    /// Recover the `f32` a quantized parameter stands for — the value fed to
    /// the bake shader's uniform, so the tile matches the node's live path.
    pub fn dequantize(q: u32) -> f32 {
        q as f32 / Self::QUANTUM
    }

    fn label(&self) -> &'static str {
        match self {
            BakeKind::Noise { .. } => "noise",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_scales_with_octaves_and_clamps() {
        // Low octaves hit the 512 floor; high octaves the 2048 ceiling;
        // every result is a sane power-of-two-ish tile edge.
        assert_eq!(BakeSpec::resolution_for_octaves(1), 512);
        assert_eq!(BakeSpec::resolution_for_octaves(4), 512);
        assert_eq!(BakeSpec::resolution_for_octaves(8), 2048);
        // Out-of-range octaves are clamped, never panic (no overflow shift).
        assert_eq!(BakeSpec::resolution_for_octaves(0), 512);
        assert_eq!(BakeSpec::resolution_for_octaves(99), 2048);
    }

    #[test]
    fn quantize_round_trips_within_quantum() {
        for v in [0.0_f32, 0.5, 0.6, 1.0, 2.5] {
            let back = BakeKind::dequantize(BakeKind::quantize(v));
            assert!((back - v).abs() <= 1.0 / BakeKind::QUANTUM, "{v} -> {back}");
        }
        // Negative inputs floor at zero (warp/roughness are clamped upstream).
        assert_eq!(BakeKind::quantize(-1.0), 0);
    }

    #[test]
    fn equal_specs_key_the_same_cache_slot() {
        // Cache dedup relies on structural equality: same field params →
        // equal spec (and equal hash), so two brushes share one tile.
        let a = BakeSpec {
            kind: BakeKind::Noise {
                seed: 3,
                octaves: 4,
                warp_q: BakeKind::quantize(0.6),
                roughness_q: BakeKind::quantize(0.5),
            },
            channels: BakeChannels::Grayscale,
            resolution: 512,
        };
        let b = a;
        assert_eq!(a, b);
        // A different channel layout is a different tile.
        let mut c = a;
        c.channels = BakeChannels::Rgba;
        assert_ne!(a, c);
    }
}
