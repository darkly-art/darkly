//! Per-node compile context, input bindings, and shader-variant tag.
//!
//! [`CompileWgslCtx`] is the state struct every node's
//! `BrushNodeEvaluator::compile_wgsl` is called with. [`NodeWgsl`] is
//! what the node returns — decls, body lines, output expressions, plus
//! any per-dab or uniform fields the node contributes. [`InputBinding`]
//! resolves how a port shows up in emitted WGSL (a substituted upstream
//! expression vs. a literalized default). [`ShaderMode`] tags which of
//! the two assembled shader variants the compiler is producing.

use std::collections::{HashMap, HashSet};

use crate::brush::wgsl::type_system::{DabField, UniformField};
use crate::brush::wire::{BrushWireType, ScalarValue};
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
    /// Port is disconnected — embed this literal value as a WGSL
    /// constant.
    Default(ScalarValue),
}

impl InputBinding {
    /// Emit the WGSL expression for this binding as an `f32`. Wired
    /// expressions assumed already-f32; `Default(Scalar/Int/Bool)`
    /// emits a literal.
    pub fn as_f32(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => format!("{:.6}", v.as_f32()),
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
                let [x, y] = v.as_vec2();
                format!("vec2<f32>({:.6}, {:.6})", x, y)
            }
        }
    }

    /// Emit as `vec4<f32>` (color/vec4).
    pub fn as_vec4(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => {
                let [r, g, b, a] = v.as_color();
                format!("vec4<f32>({:.6}, {:.6}, {:.6}, {:.6})", r, g, b, a)
            }
        }
    }
}

// ── Compile context ─────────────────────────────────────────────────────

/// Per-node context passed to `compile_wgsl`.
pub struct CompileWgslCtx<'a> {
    pub node_id: NodeId,
    pub params: &'a [crate::gpu::params::ParamValue],
    pub port_defs: &'a [PortDef<BrushWireType>],
    pub inputs: HashMap<String, InputBinding>,
    /// Curve LUT, if this node has a `Curve` parameter.
    pub lut: Option<&'a crate::brush::curve_math::CurveLut>,
    /// Output port names that have at least one downstream consumer
    /// in the graph. Nodes whose outputs are produced into the dab
    /// record (pen_input, random) only need to emit fields for
    /// consumed ports — unwired outputs cost nothing.
    pub consumed_outputs: HashSet<String>,
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
                return InputBinding::Default(ScalarValue::Scalar(port.default));
            }
        }
        InputBinding::Default(ScalarValue::Scalar(0.0))
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
    Preview,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_binding_emits_default_literal() {
        let b = InputBinding::Default(ScalarValue::Scalar(0.5));
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
