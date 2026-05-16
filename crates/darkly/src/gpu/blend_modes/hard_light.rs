use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "hard_light",
        display_name: "Hard Light",
        category: "Contrast",
        gpu_value: 10,
        wgsl_math: "\
            let lo = 2.0 * fg.rgb * bg.rgb; \
            let hi = 1.0 - 2.0 * (1.0 - fg.rgb) * (1.0 - bg.rgb); \
            Cs = select(hi, lo, fg.rgb <= vec3f(0.5));",
    }
}
