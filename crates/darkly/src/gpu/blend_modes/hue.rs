use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "hue",
        display_name: "Hue",
        category: "Component",
        gpu_value: 12,
        // PDF 11.3.5.3 / W3C Compositing-1, Krita's HSY model.
        wgsl_math: "Cs = pd_set_lum(pd_set_sat(fg.rgb, pd_sat(bg.rgb)), pd_lum(bg.rgb));",
    }
}
