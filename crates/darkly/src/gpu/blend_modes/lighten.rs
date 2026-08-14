use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "lighten",
        display_name: "Lighten",
        description: "Keeps whichever of the two colors is lighter, channel by channel.",
        category: "Lighten",
        gpu_value: 4,
        wgsl_math: "Cs = max(fg.rgb, bg.rgb);",
    }
}
