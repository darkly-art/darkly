use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "color_dodge",
        display_name: "Color Dodge",
        category: "Lighten",
        gpu_value: 6,
        // pd_color_dodge: Krita KoCompositeOpFunctions.h:376–403.
        wgsl_math: "Cs = pd_color_dodge(fg.rgb, bg.rgb);",
    }
}
