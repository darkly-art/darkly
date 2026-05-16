use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "normal",
        display_name: "Normal",
        category: "Normal",
        gpu_value: 0,
        wgsl_math: "Cs = fg.rgb;",
    }
}
