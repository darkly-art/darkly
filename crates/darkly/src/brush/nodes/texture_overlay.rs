//! Texture overlay GPU node.
//!
//! Takes a dab texture and a pattern texture, tiles the pattern in canvas
//! space, and blends it onto the dab.  This produces pencil grain, canvas
//! texture, charcoal roughness, and similar effects.
//!
//! The pattern tiles globally in canvas coordinates so the grain is
//! consistent across dabs — matching Krita's `KisTextureOption` behaviour.
//!
//! Sits between the stamp node and color_output in the graph:
//!   stamp.dab → texture_overlay.dab
//!   image("pattern.png") → texture_overlay.pattern
//!   texture_overlay.dab → color_output.dab

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::TexOverlayUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "texture_overlay",
        category: "texture",
        display_name: "Texture Overlay",
        ports: vec![
            PortDef::input("dab", BrushWireType::Texture)
                .with_description("The dab texture to apply grain to"),
            PortDef::input("pattern", BrushWireType::Texture)
                .with_description("Pattern/grain texture (tiled in canvas space)"),
            PortDef::input("dab_size", BrushWireType::Vec2)
                .with_description("Actual dab dimensions in pixels"),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Canvas position for pattern tiling alignment"),
            PortDef::input("scale", BrushWireType::Scalar)
                .with_range(0.01, 4.0, 1.0)
                .with_natural_range(0.01, 4.0)
                .with_label("Scale")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                .exposed()
                .with_description("Pattern scale (100% = natural size)"),
            PortDef::input("strength", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_natural_range(0.0, 1.0)
                .with_label("Strength")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-mountain")
                .exposed()
                .with_description("Texture blend strength (0% = none, 100% = full)"),
            PortDef::output("dab", BrushWireType::Texture).with_description("The textured dab"),
            PortDef::output("dab_size", BrushWireType::Vec2)
                .with_description("Pass-through dab dimensions"),
        ],
        params: &[
            // Enum stored as Int — see `BLEND_*` constants in the shader
            // and the match arm in evaluate_gpu. Labeled dropdown so users
            // pick by name rather than memorizing indices.
            ParamDef::Enum {
                name: "blend_mode",
                options: &["Multiply", "Subtract", "Overlay"],
                default: 0,
            },
        ],
        is_gpu: true,
    }
}

pub struct TextureOverlayEvaluator;

impl BrushNodeEvaluator for TextureOverlayEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        // Both dab and pattern must be connected.
        let dab_handle = match ctx.input("dab") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let pattern_handle = match ctx.input("pattern") {
            ScalarValue::Texture(h) => h,
            _ => {
                // No pattern connected — pass dab through unchanged.
                let dab_size = ctx.input("dab_size");
                return vec![
                    ("dab".into(), ScalarValue::Texture(dab_handle)),
                    ("dab_size".into(), dab_size),
                ];
            }
        };

        let dab_size = ctx.input("dab_size").as_vec2();
        let position = ctx.input("position").as_vec2();
        let scale = ctx.input_f32("scale").max(0.01);
        let strength = ctx.input_f32("strength");

        let blend_mode = match ctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => *v as u32,
            _ => 0,
        };

        let dab_w = dab_size[0] as u32;
        let dab_h = dab_size[1] as u32;
        if dab_w == 0 || dab_h == 0 {
            return vec![];
        }

        // Get pattern dimensions for tiling calculation.
        let (pattern_w, pattern_h) = gpu.dab_pool.texture_size(pattern_handle);

        // Acquire a new dab texture for the textured output.
        let out_handle = gpu.dab_pool.acquire(gpu.device);
        let out_view = gpu.dab_pool.view(out_handle);

        // Write uniforms.
        let uniforms = TexOverlayUniforms {
            dab_width: dab_w as f32,
            dab_height: dab_h as f32,
            position_x: position[0],
            position_y: position[1],
            pattern_width: pattern_w as f32,
            pattern_height: pattern_h as f32,
            scale,
            strength,
            blend_mode,
            _pad: [0.0; 3],
        };
        let offset = gpu
            .pipelines
            .write_tex_overlay_uniforms(gpu.queue, &uniforms);

        // Get bind groups for sampling the dab and pattern textures.
        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle);
        let pattern_bind_group = gpu.dab_pool.bind_group(pattern_handle);

        // Render textured dab.
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-tex-overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: out_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            pass.set_viewport(0.0, 0.0, dab_w as f32, dab_h as f32, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.tex_overlay_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.tex_overlay_uniform_bind_group, &[offset]);
            pass.set_bind_group(1, dab_bind_group, &[]);
            pass.set_bind_group(2, pattern_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        vec![
            ("dab".into(), ScalarValue::Texture(out_handle)),
            ("dab_size".into(), ScalarValue::Vec2(dab_size)),
        ]
    }
}
