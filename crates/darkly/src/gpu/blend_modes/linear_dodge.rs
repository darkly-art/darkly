use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "linear_dodge",
        display_name: "Linear Dodge (Add)",
        description: "Adds the two colours together, blowing out to white quickly.",
        category: "Lighten",
        gpu_value: 7,
        wgsl_math: "Cs = clamp(fg.rgb + bg.rgb, vec3f(0.0), vec3f(1.0));",
    }
}
