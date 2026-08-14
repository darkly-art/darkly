use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "selectAll",
        display_name: "Select All",
        description: "Select the entire canvas.",
        icon: "fa6-solid:vector-square",
    },
    ActionDef {
        id: "clearSelection",
        display_name: "Deselect",
        description: "Clear the active selection.",
        icon: "fa6-solid:ban",
    },
    ActionDef {
        id: "invertSelection",
        display_name: "Invert Selection",
        description: "Invert the current selection.",
        icon: "tabler:flip-horizontal",
    },
    ActionDef {
        id: "maskToSelection",
        display_name: "Mask to Selection",
        description: "Load the active layer's mask as the selection.",
        icon: "radix-icons:mask-off",
    },
    ActionDef {
        id: "clearSelectionContents",
        display_name: "Clear Selection Contents",
        description: "Erase the pixels inside the selection.",
        icon: "fa6-solid:eraser",
    },
    ActionDef {
        id: "growSelection",
        display_name: "Grow Selection",
        description: "Expand the selection edge outward by a number of pixels.",
        icon: "fa6-solid:up-right-and-down-left-from-center",
    },
    ActionDef {
        id: "shrinkSelection",
        display_name: "Shrink Selection",
        description: "Contract the selection edge inward by a number of pixels.",
        icon: "fa6-solid:down-left-and-up-right-to-center",
    },
    ActionDef {
        id: "borderSelection",
        display_name: "Border Selection",
        description: "Replace the selection with a band straddling its edge.",
        icon: "fa6-solid:border-all",
    },
    ActionDef {
        id: "smoothSelection",
        display_name: "Smooth Selection",
        description: "Round off jagged edges and remove small specks.",
        icon: "fa6-solid:wand-magic-sparkles",
    },
    ActionDef {
        id: "featherSelection",
        display_name: "Feather Selection",
        description: "Soften the selection edge with a Gaussian blur.",
        icon: "fa6-solid:feather",
    },
    ActionDef {
        id: "antialiasSelection",
        display_name: "Antialias Selection",
        description: "Soften the staircase of a hard-edged selection.",
        icon: "fa6-solid:wand-magic",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "selection",
        actions: ACTIONS,
    }
}
