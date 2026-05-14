use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "overlay",
        display_name: "Overlay",
        category: "Contrast",
        gpu_value: 8,
        wgsl_math: "\
            let lo = 2.0 * fg.rgb * bg.rgb; \
            let hi = 1.0 - 2.0 * (1.0 - fg.rgb) * (1.0 - bg.rgb); \
            Cs = select(hi, lo, bg.rgb < vec3f(0.5));",
    }
}
