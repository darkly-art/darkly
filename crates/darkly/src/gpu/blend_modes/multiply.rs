use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "multiply",
        display_name: "Multiply",
        category: "Darken",
        gpu_value: 2,
        wgsl_math: "Cs = fg.rgb * bg.rgb;",
    }
}
