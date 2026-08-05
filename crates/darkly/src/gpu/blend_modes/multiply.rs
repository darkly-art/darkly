use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "multiply",
        display_name: "Multiply",
        description:
            "Multiplies the two colours, darkening everywhere and keeping white transparent.",
        category: "Darken",
        gpu_value: 2,
        wgsl_math: "Cs = fg.rgb * bg.rgb;",
    }
}
