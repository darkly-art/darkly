use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "darken",
        display_name: "Darken",
        description: "Keeps whichever of the two colours is darker, channel by channel.",
        category: "Darken",
        gpu_value: 1,
        wgsl_math: "Cs = min(fg.rgb, bg.rgb);",
    }
}
