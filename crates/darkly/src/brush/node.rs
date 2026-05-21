//! Brush-flavoured wrapper around the generic [`NodeRegistration`].
//!
//! Each brush node module's `register()` returns a [`BrushNodeRegistration`].
//! Compared to the bare nodegraph registration, this wrapper also carries
//! the GPU pipeline registrations the node owns (zero, one, or more).
//!
//! The nodegraph compiler only knows about [`NodeRegistration<W>`]; the
//! brush layer unwraps `.node` when feeding it into the compiler.  The
//! brush pipeline registry harvests `.pipelines` from every node at
//! [`BrushPipelines::new`](super::pipeline::BrushPipelines::new) time.

use crate::nodegraph::NodeRegistration;

use super::pipeline::BrushPipelineRegistration;
use super::wire::BrushWireType;

/// A brush node's static metadata plus the GPU pipelines it owns.
///
/// Compute nodes (add, clamp, mix, …) leave `pipelines` empty.  Terminal
/// nodes that talk to the GPU (stamp, liquify, watercolor, …) declare one
/// or more pipelines that the central [`BrushPipelines`](super::pipeline::BrushPipelines)
/// registry will build at engine init.
#[derive(Clone)]
pub struct BrushNodeRegistration {
    pub node: NodeRegistration<BrushWireType>,
    pub pipelines: Vec<BrushPipelineRegistration>,
}

impl BrushNodeRegistration {
    /// Construct a compute-only node (no GPU pipelines).
    pub fn compute(node: NodeRegistration<BrushWireType>) -> Self {
        Self {
            node,
            pipelines: Vec::new(),
        }
    }
}

impl std::ops::Deref for BrushNodeRegistration {
    type Target = NodeRegistration<BrushWireType>;
    fn deref(&self) -> &Self::Target {
        &self.node
    }
}
