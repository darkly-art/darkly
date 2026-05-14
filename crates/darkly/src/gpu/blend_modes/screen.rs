use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "screen",
        display_name: "Screen",
        category: "Lighten",
        gpu_value: 5,
        wgsl_math: "Cs = fg.rgb + bg.rgb - fg.rgb * bg.rgb;",
    }
}
