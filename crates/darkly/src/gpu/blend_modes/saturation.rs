use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "saturation",
        display_name: "Saturation",
        description:
            "Takes the saturation of this layer and the hue and brightness of what is beneath.",
        category: "Component",
        gpu_value: 13,
        wgsl_math: "Cs = pd_set_lum(pd_set_sat(bg.rgb, pd_sat(fg.rgb)), pd_lum(bg.rgb));",
    }
}
