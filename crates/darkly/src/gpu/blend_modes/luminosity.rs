use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "luminosity",
        display_name: "Luminosity",
        description:
            "Takes the brightness of this layer and the hue and saturation of what is beneath.",
        category: "Component",
        gpu_value: 15,
        wgsl_math: "Cs = pd_set_lum(bg.rgb, pd_lum(fg.rgb));",
    }
}
