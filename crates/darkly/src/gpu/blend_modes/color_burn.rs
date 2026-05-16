use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "color_burn",
        display_name: "Color Burn",
        category: "Darken",
        gpu_value: 3,
        // pd_color_burn: Krita KoCompositeOpFunctions.h:329–361.
        // Helper lives in the composite shader prelude.
        wgsl_math: "Cs = pd_color_burn(fg.rgb, bg.rgb);",
    }
}
