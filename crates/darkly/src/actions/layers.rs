use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "addLayer",
        display_name: "Add Layer…",
        description: "Open the add-layer picker — normal, filter, veil, void or group.",
        icon: "fa6-solid:plus",
    },
    ActionDef {
        id: "newLayer",
        display_name: "New Layer",
        description: "Add a new layer above the active one.",
        icon: "fa6-solid:square-plus",
    },
    ActionDef {
        id: "newFilterLayer",
        display_name: "New Filter Layer",
        description: "Add a non-destructive filter layer (curves, levels, invert, …) above the active one.",
        icon: "fa6-solid:circle-half-stroke",
    },
    ActionDef {
        id: "newVeil",
        display_name: "New Veil",
        description: "Add a veil — a post-process effect (rainy glass, VHS, grain, …) over the whole canvas.",
        icon: "material-symbols:curtains-rounded",
    },
    ActionDef {
        id: "newVoid",
        display_name: "New Void",
        description: "Add a void — a layer filled from a procedural or live source (noise, camera, screen share, …).",
        icon: "tabler:galaxy",
    },
    ActionDef {
        id: "newGroup",
        display_name: "New Group",
        description: "Group the selected layers together, or add an empty group if nothing is selected.",
        icon: "fa6-solid:folder-plus",
    },
    ActionDef {
        id: "duplicateLayer",
        display_name: "Duplicate Layer",
        description: "Make a copy of each selected layer.",
        icon: "fa6-solid:clone",
    },
    ActionDef {
        id: "deleteLayer",
        display_name: "Delete Layer",
        description: "Delete the selected layers.",
        icon: "fa6-solid:trash",
    },
    ActionDef {
        id: "flipLayerH",
        display_name: "Flip Horizontally",
        description: "Mirror the active layer (or selection) left-to-right.",
        icon: "fa6-solid:arrows-left-right",
    },
    ActionDef {
        id: "flipLayerV",
        display_name: "Flip Vertically",
        description: "Mirror the active layer (or selection) top-to-bottom.",
        icon: "fa6-solid:arrows-up-down",
    },
    ActionDef {
        id: "toggleVisibility",
        display_name: "Toggle Layer Visibility",
        description: "Show or hide the active layer.",
        icon: "fa6-solid:eye",
    },
    ActionDef {
        id: "toggleLock",
        display_name: "Toggle Layer Lock",
        description: "Lock or unlock the active layer.",
        icon: "fa6-solid:lock",
    },
    ActionDef {
        id: "isolateLayer",
        display_name: "Isolate Layer",
        description: "Solo a layer so only it shows in the canvas. Press again to bring everything else back.",
        icon: "fa6-solid:circle-dot",
    },
    ActionDef {
        id: "addMask",
        display_name: "Add Mask",
        description: "Add a mask modifier to the active layer or group and activate it for painting.",
        icon: "radix-icons:mask-on",
    },
    ActionDef {
        id: "mergeDown",
        display_name: "Merge Down",
        description: "Merge the active layer into the one below it, or combine multiple selected layers into a single layer.",
        icon: "fa6-solid:arrows-down-to-line",
    },
    ActionDef {
        id: "flatten",
        display_name: "Flatten",
        description: "Bake a layer into plain pixels: apply its mask, rasterize a generated layer (smart object, camera, text) so it can be painted on, or flatten a group into a single raster that inherits the group’s blend props.",
        icon: "fa6-solid:layer-group",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "layers",
        actions: ACTIONS,
    }
}
