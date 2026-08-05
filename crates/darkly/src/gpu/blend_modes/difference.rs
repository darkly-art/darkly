use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "difference",
        display_name: "Difference",
        description:
            "Subtracts the darker colour from the lighter one, inverting where they differ.",
        category: "Inversion",
        gpu_value: 11,
        wgsl_math: "Cs = abs(fg.rgb - bg.rgb);",
    }
}
