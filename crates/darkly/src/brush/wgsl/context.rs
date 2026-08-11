//! Per-node compile context, input bindings, and shader-variant tag.
//!
//! [`CompileWgslCtx`] is the state struct every node's
//! `BrushNodeEvaluator::compile_wgsl` is called with. [`NodeWgsl`] is
//! what the node returns — decls, body lines, output expressions, plus
//! any per-dab or uniform fields the node contributes. [`InputBinding`]
//! resolves how a port shows up in emitted WGSL (a substituted upstream
//! expression vs. a literalized default). [`ShaderMode`] tags which of
//! the two assembled shader variants the compiler is producing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::brush::input_value::InputValue;
use crate::brush::texture_source::{LiveSource, ResolvedSource};
use crate::brush::wgsl::type_system::{DabField, UniformField};
use crate::brush::wire::BrushWireType;
use crate::nodegraph::{NodeId, PortDef, PortDir};

// ── Per-node compile output ─────────────────────────────────────────────

/// What one node contributes to the compiled fragment shader.
#[derive(Default, Clone, Debug)]
pub struct NodeWgsl {
    /// Module-scope WGSL declarations: helper functions, const arrays,
    /// structs. Concatenated into the shader before `fs_main`.
    pub decls: String,
    /// Lines inserted into the `fs_main` body, in topological order.
    /// May reference: `d` (the `DabRecord`), `u` (the `Uniforms`),
    /// `local_uv: vec2<f32>` (fragment offset from dab centre, normalized
    /// so the unmodulated disc edge is at `length = 1`), `local_dist: f32`
    /// (= `length(local_uv)`), `theta: f32` (= `atan2(local_uv.y, local_uv.x)`),
    /// `target_pos: vec2<f32>` (fragment's position in the target
    /// texture's pixel space — canvas px for stroke, mask texels for
    /// preview), and any function declared in `decls` or by upstream
    /// nodes.
    pub body: String,
    /// Output port name → a WGSL expression downstream nodes substitute
    /// for that port's value. Typically a `let`-binding name introduced
    /// by `body`, but may also be a dab-field reference (`d.foo`), a
    /// uniform reference (`u.foo`), or a literal.
    pub outputs: HashMap<String, String>,
    /// Per-dab record fields this node contributes.
    pub dab_fields: Vec<DabField>,
    /// Stroke-constant uniform fields this node contributes.
    pub uniform_fields: Vec<UniformField>,
    /// Extra `@group(...) @binding(...) var ...` declarations the
    /// terminal node owns. Spliced into the assembled shader after
    /// the framework's three intrinsic bind groups (group 0: uniforms,
    /// group 1: dabs, group 2: selection). Only the terminal node
    /// should set this — the per-brush pipeline build must match the
    /// declared layout. Empty for every non-terminal node.
    ///
    /// Use case: terminals like `watercolor` need bindings
    /// the standard fragment-stage prelude doesn't provide (pickup
    /// atlas, pre-stroke canvas). Declaring them here keeps the
    /// extension scoped to the one node that uses it instead of
    /// extending the `BrushNodeEvaluator` trait surface.
    pub terminal_bindings: String,
}

// ── Input binding ───────────────────────────────────────────────────────

/// How an input port resolves when emitting WGSL.
#[derive(Clone, Debug)]
pub enum InputBinding {
    /// Port is wired to an upstream output — substitute this WGSL
    /// expression at every use site.
    Wired(String),
    /// Port is disconnected — embed this authored value as a WGSL
    /// constant (or read it as a compile-time enum/string/curve value).
    Default(InputValue),
}

impl InputBinding {
    /// Emit the WGSL expression for this binding as an `f32`. Wired
    /// expressions are assumed already-f32; a disconnected scalar-family
    /// value emits its `{:.6}` literal.
    pub fn as_f32(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => format!("{:.6}", v.as_f32()),
        }
    }

    /// The concrete `f32` value of an unwired (default) binding, or `None`
    /// when the input is wired (a per-dab expression with no compile-time
    /// value). Use to read a static parameter as an actual number — e.g. to
    /// build a compile-time bake-spec key — rather than as a WGSL string.
    pub fn as_f32_literal(&self) -> Option<f32> {
        match self {
            Self::Wired(_) => None,
            Self::Default(v) => Some(v.as_f32()),
        }
    }

    /// Emit as `u32`. Coerces literals; wired exprs get a runtime cast.
    pub fn as_u32(&self) -> String {
        match self {
            Self::Wired(expr) => format!("u32({})", expr),
            Self::Default(v) => format!("{}u", v.as_f32().max(0.0) as u32),
        }
    }

    /// Emit as `vec2<f32>`.
    pub fn as_vec2(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => {
                let [x, y] = v.as_scalar_value().as_vec2();
                format!("vec2<f32>({:.6}, {:.6})", x, y)
            }
        }
    }

    /// Emit as `vec4<f32>` (color/vec4).
    pub fn as_vec4(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => {
                let [r, g, b, a] = v.as_scalar_value().as_color();
                format!("vec4<f32>({:.6}, {:.6}, {:.6}, {:.6})", r, g, b, a)
            }
        }
    }

    /// Read a disconnected input's compile-time enum / branch-selector
    /// index. Enum inputs are non-wirable (the connect guard rejects a wire),
    /// so a `Wired` binding never reaches here — it falls back to `0`.
    pub fn enum_index(&self) -> i32 {
        match self {
            Self::Default(v) => v.as_enum_index(),
            Self::Wired(_) => 0,
        }
    }

    /// Read a disconnected input's boolean flag (compile-time constant).
    pub fn boolean(&self) -> bool {
        match self {
            Self::Default(v) => v.as_bool(),
            Self::Wired(_) => false,
        }
    }

    /// Read a disconnected input's string value (texture / icon name). Owned
    /// so callers don't fight the `InputBinding`'s lifetime; String inputs
    /// are non-wirable, so a `Wired` binding never reaches here.
    pub fn string(&self) -> String {
        match self {
            Self::Default(v) => v.as_str().to_string(),
            Self::Wired(_) => String::new(),
        }
    }
}

// ── Compile context ─────────────────────────────────────────────────────

/// Per-node context passed to `compile_wgsl`.
pub struct CompileWgslCtx<'a> {
    pub node_id: &'a NodeId,
    pub port_defs: &'a [PortDef<BrushWireType>],
    pub inputs: HashMap<String, InputBinding>,
    /// Curve LUT, if this node has a `Curve` input.
    pub lut: Option<&'a crate::brush::curve_math::CurveLut>,
    /// Output port names that have at least one downstream consumer
    /// in the graph. Nodes whose outputs are produced into the dab
    /// record (pen_input, random) only need to emit fields for
    /// consumed ports — unwired outputs cost nothing.
    pub consumed_outputs: HashSet<String>,
    /// Shared, ordered, deduped accumulator of the `@group(3)` texture
    /// slots the graph's nodes request. Each entry is a
    /// [`ResolvedSource`] — a named registry texture (`image`) or a
    /// baked procedural tile (`noise`). Mutated through
    /// [`Self::request_source`] / [`Self::request_texture`]; the compiler
    /// reads the final list out after walking every node and copies it
    /// onto [`crate::brush::wgsl::CompiledBrush::graph_sources`].
    /// `RefCell` so `compile_wgsl(&self)` can append without forcing a
    /// `&mut CompileWgslCtx` rewrite across every existing node.
    pub graph_sources: &'a RefCell<Vec<ResolvedSource>>,
}

impl CompileWgslCtx<'_> {
    /// Look up an input binding, falling back to the port's default
    /// when disconnected. The default is materialised as a literal in
    /// the emitted WGSL.
    pub fn input(&self, name: &str) -> InputBinding {
        if let Some(b) = self.inputs.get(name) {
            return b.clone();
        }
        for port in self.port_defs {
            if port.name == name && port.dir == PortDir::Input {
                return InputBinding::Default(port.value.clone());
            }
        }
        InputBinding::Default(InputValue::Scalar(0.0))
    }

    /// Returns `true` if a connected wire targets this input port
    /// (i.e. not falling through to the port default). Useful for
    /// nodes whose output depends on whether an input was supplied.
    pub fn input_is_wired(&self, name: &str) -> bool {
        matches!(self.inputs.get(name), Some(InputBinding::Wired(_)))
    }

    /// Suffix an identifier with this node's id so per-node WGSL
    /// symbols don't collide.
    pub fn ident(&self, base: &str) -> String {
        format!("{}_{}", base, self.node_id.0)
    }

    /// Suffix a dab-record field name with this node's id. Use for
    /// every per-dab field so two instances of the same node type
    /// don't collide in the generated `DabRecord` struct.
    pub fn dab_field_name(&self, base: &str) -> String {
        format!("n{}_{}", self.node_id.0, base)
    }

    /// Suffix a uniform field name with this node's id.
    pub fn uniform_field_name(&self, base: &str) -> String {
        format!("n{}_{}", self.node_id.0, base)
    }

    /// Reserve (or look up) a `@group(3)` binding slot for a resolved
    /// texture source. Returns the slot index — `0` for the first
    /// distinct source in the graph, `1` for the second, and so on.
    /// Re-requesting an equal source (same name, or same [`BakeSpec`])
    /// returns the existing slot, so brushes that reference one field
    /// twice bind it once.
    ///
    /// Use the returned index to reference the texture in emitted WGSL
    /// as `graph_tex_{slot}` (the shared sampler is always `graph_smp`).
    /// The compiler resolves each source at per-brush pipeline-build
    /// time — [`ResolvedSource::Named`] against the
    /// [`crate::gpu::texture_registry::TextureRegistry`],
    /// [`ResolvedSource::Baked`] against the bake cache.
    pub fn request_source(&self, source: ResolvedSource) -> u32 {
        let mut list = self.graph_sources.borrow_mut();
        if let Some(idx) = list.iter().position(|s| *s == source) {
            return idx as u32;
        }
        let idx = list.len() as u32;
        list.push(source);
        idx
    }

    /// Reserve (or look up) a slot for a named registry texture — the
    /// [`ResolvedSource::Named`] shim over [`Self::request_source`].
    pub fn request_texture(&self, name: &str) -> u32 {
        self.request_source(ResolvedSource::Named(name.to_string()))
    }

    /// Reserve (or look up) a slot for a texture the requesting node
    /// republishes every flush — the [`ResolvedSource::Live`] shim over
    /// [`Self::request_source`].
    ///
    /// Unlike [`Self::request_texture`], the view is not resolved against
    /// the [`crate::gpu::texture_registry::TextureRegistry`] at
    /// pipeline-build time; the producing node publishes it during its own
    /// `flush_dabs` and the terminal binds whatever is there. A slot with
    /// nothing published falls back to `_fallback`, which is how the
    /// cursor preview renders without a stroke.
    pub fn request_live_texture(&self, live: LiveSource) -> u32 {
        self.request_source(ResolvedSource::Live(live))
    }
}

// ── Shader mode ─────────────────────────────────────────────────────────

/// Which of the two compiled shader variants is being assembled.
///
/// The upstream graph contributes the same per-fragment shape /
/// color / flow expressions in both modes — only the outer skeleton
/// differs:
///
/// - **`Stroke`**: instanced quad-per-dab vertex stage; `sel` sampled
///   from a bound selection texture; terminal `@group(3)` bindings
///   (scratch mirror, pickup atlas) declared.
/// - **`Preview`**: single quad centred at `preview_centre`; `sel = 1.0`
///   inlined; no `@group(2)` selection binding, no `@group(3)`
///   terminal bindings.
///
/// The two modes share `node_decls`, `dab_layout`, and
/// `uniform_layout` — every brush stores both WGSL strings side-by-side
/// on [`crate::brush::wgsl::CompiledBrush`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ShaderMode {
    Stroke,
    CursorPreview,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_binding_emits_default_literal() {
        let b = InputBinding::Default(InputValue::Scalar(0.5));
        assert!(b.as_f32().starts_with("0.5"));
        assert!(b.as_vec2().starts_with("vec2<f32>(0.5"));
    }

    #[test]
    fn input_binding_passes_wired_through() {
        let b = InputBinding::Wired("d.foo".into());
        assert_eq!(b.as_f32(), "d.foo");
        assert_eq!(b.as_vec2(), "d.foo");
    }
}
