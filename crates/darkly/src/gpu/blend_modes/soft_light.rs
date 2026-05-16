use crate::gpu::blend_mode::BlendModeRegistration;

pub fn register() -> BlendModeRegistration {
    BlendModeRegistration {
        type_id: "soft_light",
        display_name: "Soft Light",
        category: "Contrast",
        gpu_value: 9,
        // pd_soft_light: Photoshop variant, Krita KoCompositeOpFunctions.h:513–529.
        wgsl_math: "Cs = pd_soft_light(fg.rgb, bg.rgb);",
    }
}
