use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "lighten",
        display_name: "Lighten",
        category: "Lighten",
        gpu_value: 4,
        wgsl_math: "Cs = max(fg.rgb, bg.rgb);",
    }
}
