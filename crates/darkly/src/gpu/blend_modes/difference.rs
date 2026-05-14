use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "difference",
        display_name: "Difference",
        category: "Inversion",
        gpu_value: 11,
        wgsl_math: "Cs = abs(fg.rgb - bg.rgb);",
    }
}
