//! Image source GPU node.
//!
//! Outputs a cached brush tip texture as a `Texture` handle.  The image
//! is uploaded to the `DabTexturePool` tip cache at preset load time;
//! this node just looks it up by `resource_name` and passes the handle
//! downstream (e.g. to a stamp node).
//!
//! No render passes are recorded — this is a pure data-source node that
//! lives in the GPU phase because it needs access to the dab pool.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "image",
        category: "gpu",
        display_name: "Image",
        ports: vec![PortDef::output("texture", BrushWireType::Texture)
            .with_description("The loaded brush tip image as a GPU texture")],
        params: &[ParamDef::String {
            name: "resource_name",
            default: "",
        }],
        is_gpu: true,
    }
}

pub struct ImageEvaluator;

impl BrushNodeEvaluator for ImageEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let resource_name = ctx.param_str(0);
        let Some(&handle) = gpu.resource_handles.get(resource_name) else {
            log::warn!("image node: resource '{}' not uploaded", resource_name);
            return vec![];
        };
        vec![("texture".into(), ScalarValue::Texture(handle))]
    }
}
