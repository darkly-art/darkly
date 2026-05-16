use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "color",
        display_name: "Color",
        category: "Component",
        gpu_value: 14,
        wgsl_math: "Cs = pd_set_lum(fg.rgb, pd_lum(bg.rgb));",
    }
}
