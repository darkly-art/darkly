//! Brush-flavoured wrapper around the generic [`NodeRegistration`].
//!
//! Each brush node module's `register()` returns a [`BrushNodeRegistration`].
//! Compared to the bare nodegraph registration, this wrapper also carries:
//!
//! - the GPU pipeline registrations the node owns (zero, one, or more),
//! - a constructor for the node's CPU/GPU evaluator (a trait object), and
//! - the node's stroke-lifecycle hook (clear scratch / seed from pre-stroke / none).
//!
//! Bundling the evaluator constructor here is the load-bearing design choice
//! that lets [`crate::brush::BrushNodeRegistry`] be the single source of
//! truth for "what nodes exist?" — there is no parallel hand-written
//! evaluator map to keep in sync. See AGENTS.md "Modularity Principle".
//!
//! The nodegraph compiler only knows about [`NodeRegistration<W>`]; the
//! brush layer unwraps `.node` when feeding it into the compiler.  The
//! brush pipeline registry harvests `.pipelines` from every node at
//! [`BrushPipelines::new`](super::pipeline::BrushPipelines::new) time.

use crate::nodegraph::NodeRegistration;

use super::eval::BrushNodeEvaluator;
use super::pipeline::BrushPipelineRegistration;
use super::wire::BrushWireType;

/// Stroke-scoped scratch setup the framework runs before a terminal's
/// `begin_stroke` hook fires. Every terminal that touches the scratch
/// declares its lifecycle here — copy-pasted prologues in each terminal's
/// `begin_stroke` impl used to drift (see the watercolor regression fixed
/// by 24ccdcf), so the prologue is framework-owned now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// No framework-managed scratch prep. Non-terminal nodes and
    /// terminals that don't touch the stroke scratch.
    None,
    /// Clear `scratch.write_view` to transparent. Used by terminals
    /// whose source-over composite accumulates from zero (paint,
    /// watercolor).
    ClearScratchToTransparent,
    /// Seed `scratch.write_texture` from `pre_stroke_texture` via
    /// `copy_texture_to_texture`. Used by terminals whose commit
    /// blits the whole scratch back onto the layer, so unchanged
    /// pixels must reproduce the pre-stroke state (smudge, liquify).
    SeedScratchFromPreStroke,
}

/// A brush node's static metadata plus the GPU pipelines, evaluator
/// constructor, and stroke-lifecycle hook it owns.
///
/// Compute nodes (add, clamp, mix, …) leave `pipelines` empty and set
/// `lifecycle = Lifecycle::None`. Terminal nodes that talk to the GPU
/// (stamp, liquify, watercolor, …) declare one or more pipelines and a
/// terminal-appropriate lifecycle.
#[derive(Clone)]
pub struct BrushNodeRegistration {
    pub node: NodeRegistration<BrushWireType>,
    pub pipelines: Vec<BrushPipelineRegistration>,
    /// Constructor for this node type's evaluator. The registry calls
    /// this once per `evaluators()` invocation — evaluators are trait
    /// objects (not `Clone`), so the registry can't memoize a single
    /// instance.
    pub evaluator: fn() -> Box<dyn BrushNodeEvaluator>,
    /// Framework-managed stroke prologue. See [`Lifecycle`].
    pub lifecycle: Lifecycle,
    /// Texel format of the stroke scratch this terminal renders into.
    ///
    /// Colour terminals leave this at [`COLOR_SCRATCH_FORMAT`]. Warp
    /// terminals accumulate a displacement field rather than pixels and
    /// declare a two-channel float format instead — the scratch *is* the
    /// field, so everything that already tracks the scratch (grow, rebase,
    /// read mirror, checkpoint ring) tracks the field for free. See
    /// [`crate::brush::warp_field`].
    ///
    /// The stroke buffer pairs the format with the matching canvas-copy
    /// bind group layout via
    /// [`BrushPipelines::canvas_copy_layout_for`](crate::brush::pipeline::BrushPipelines::canvas_copy_layout_for),
    /// so a terminal never has to think about filterability.
    pub scratch_format: wgpu::TextureFormat,
}

/// Scratch format for terminals that accumulate colour — the default, and
/// what every terminal but `liquify` uses.
pub const COLOR_SCRATCH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl BrushNodeRegistration {
    /// Construct a compute-only node (no GPU pipelines, no lifecycle).
    pub fn compute(
        node: NodeRegistration<BrushWireType>,
        evaluator: fn() -> Box<dyn BrushNodeEvaluator>,
    ) -> Self {
        Self {
            node,
            pipelines: Vec::new(),
            evaluator,
            lifecycle: Lifecycle::None,
            scratch_format: COLOR_SCRATCH_FORMAT,
        }
    }

    /// The output port a per-node preview should visualise, if any — the
    /// first output the node declares as a spatial image via
    /// [`PortDef::preview_image`] (`circle.mask`, `image.color`, `noise.color`,
    /// `stamp.dab`). Sensor/math/constant outputs (`random.value`,
    /// `paint_color.color`, `multiply.result`, `pen_input.*`) leave the flag
    /// off and return `None` — they carry no spatial field a thumbnail could
    /// show. The node-preview subgraph builder
    /// ([`crate::brush::node_preview_subgraph`]) uses this to pick which
    /// output to wire into the terminal chain; a new node opts into previews
    /// by flagging its image output, no consumer edits.
    pub fn preview_output(&self) -> Option<&crate::nodegraph::PortDef<BrushWireType>> {
        self.node.ports.iter().find(|p| p.preview_image)
    }
}

impl std::ops::Deref for BrushNodeRegistration {
    type Target = NodeRegistration<BrushWireType>;
    fn deref(&self) -> &Self::Target {
        &self.node
    }
}
