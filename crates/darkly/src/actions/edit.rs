use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "undo",
        display_name: "Undo",
        description: "Undo the last action.",
        icon: "fa6-solid:rotate-left",
    },
    ActionDef {
        id: "redo",
        display_name: "Redo",
        description: "Redo the last undone action.",
        icon: "fa6-solid:rotate-right",
    },
    ActionDef {
        id: "cut",
        display_name: "Cut",
        description: "Cut the active layer to the clipboard.",
        icon: "fa6-solid:scissors",
    },
    ActionDef {
        id: "copy",
        display_name: "Copy",
        description: "Copy the active layer to the clipboard.",
        icon: "fa6-solid:copy",
    },
    ActionDef {
        id: "paste",
        display_name: "Paste",
        description: "Paste an image or layer from the clipboard.",
        icon: "fa6-solid:paste",
    },
    ActionDef {
        id: "pasteAsSmartObject",
        display_name: "Paste as Smart Object",
        description: "Paste the clipboard image as a smart object: a layer you can resize freely without losing quality, because the original is kept and re-sampled rather than overwritten.",
        icon: "tabler:photo-scan",
    },
    ActionDef {
        id: "pasteInPlace",
        display_name: "Paste in Place",
        description: "Paste from the clipboard at its original position.",
        icon: "fa6-solid:clipboard",
    },
    ActionDef {
        id: "resizeCanvas",
        display_name: "Resize Canvas",
        description: "Resize the canvas with a 9-point anchor.",
        icon: "fa6-solid:up-right-and-down-left-from-center",
    },
    ActionDef {
        id: "rescaleImage",
        display_name: "Scale Image to New Size",
        description: "Resize all layers to new document dimensions.",
        icon: "fa6-solid:expand",
    },
    ActionDef {
        id: "cropToSelection",
        display_name: "Crop to Selection",
        description: "Crop the canvas to the current selection bounds.",
        icon: "fa6-solid:crop-simple",
    },
    ActionDef {
        id: "flipCanvasH",
        display_name: "Flip Canvas Horizontally",
        description: "Mirror the whole canvas left-to-right.",
        icon: "fa6-solid:arrows-left-right",
    },
    ActionDef {
        id: "flipCanvasV",
        display_name: "Flip Canvas Vertically",
        description: "Mirror the whole canvas top-to-bottom.",
        icon: "fa6-solid:arrows-up-down",
    },
    ActionDef {
        id: "rotateCanvasCW",
        display_name: "Rotate Canvas 90° CW",
        description: "Rotate the whole canvas a quarter turn clockwise.",
        icon: "fa6-solid:rotate-right",
    },
    ActionDef {
        id: "rotateCanvasCCW",
        display_name: "Rotate Canvas 90° CCW",
        description: "Rotate the whole canvas a quarter turn counter-clockwise.",
        icon: "fa6-solid:rotate-left",
    },
    ActionDef {
        id: "rotateCanvas180",
        display_name: "Rotate Canvas 180°",
        description: "Rotate the whole canvas a half turn.",
        icon: "fa6-solid:rotate",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "edit",
        actions: ACTIONS,
    }
}
