use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "brushSizeUp",
        display_name: "Increase Brush Size",
        description: "Step the active brush's size up one notch.",
        icon: "fa6-solid:plus",
    },
    ActionDef {
        id: "brushSizeDown",
        display_name: "Decrease Brush Size",
        description: "Step the active brush's size down one notch.",
        icon: "fa6-solid:minus",
    },
    ActionDef {
        id: "brushSizeAdjust",
        display_name: "Adjust Brush Size (drag)",
        description: "Hold the modifier and drag sideways to scrub the brush size continuously.",
        icon: "fa6-solid:up-right-and-down-left-from-center",
    },
    ActionDef {
        id: "setCloneSource",
        display_name: "Set Clone Source",
        description: "Hold the modifier and click on the canvas to set the point the Clone brush copies from.",
        icon: "fa6-solid:crosshairs",
    },
    ActionDef {
        id: "addBrushNode",
        display_name: "Add Brush Node",
        description: "Open the add-node menu at the cursor (brush builder).",
        icon: "fa6-solid:diagram-project",
    },
    // Both say "pack": a `.darkly-brush` file names a container, not a count,
    // the same way `.darkly` does for layers. One may hold twenty brushes.
    ActionDef {
        id: "importBrushPack",
        display_name: "Import Brush Pack…",
        description: "Import a `.darkly-brush` pack; one file may contain any number of brushes.",
        icon: "fa6-solid:file-import",
    },
    ActionDef {
        id: "exportBrushPack",
        display_name: "Export Brush Pack…",
        description: "Export one of your brush packs as a `.darkly-brush` file to share.",
        icon: "fa6-solid:file-export",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "brush",
        actions: ACTIONS,
    }
}
