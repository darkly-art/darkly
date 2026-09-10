use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "swapColors",
        display_name: "Swap Colors",
        description: "Swap the foreground and background colors.",
        icon: "fa6-solid:right-left",
    },
    ActionDef {
        id: "resetColors",
        display_name: "Reset Colors",
        description: "Reset the foreground/background to black and white.",
        icon: "fa6-solid:circle-half-stroke",
    },
    ActionDef {
        id: "sampleColor",
        display_name: "Sample Color",
        description:
            "Hold the modifier and drag on the canvas to sample a color into the foreground swatch.",
        icon: "fa6-solid:eye-dropper",
    },
    ActionDef {
        id: "palettePopup",
        display_name: "Palette Popup",
        description: "With the trigger held (a pen's barrel button by default), touch the canvas to summon a radial wheel of recent colors and brushes; drag through it and lift to pick.",
        icon: "fa6-solid:palette",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "colors",
        actions: ACTIONS,
    }
}
