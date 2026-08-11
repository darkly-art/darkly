use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "newDocument",
        display_name: "New",
        description: "Open a fresh document in a new tab. Prompts for canvas size and background color.",
        icon: "fa6-solid:file",
    },
    ActionDef {
        id: "open",
        display_name: "Open",
        description: "Open a `.darkly` document or image (PNG / JPEG / WebP) in a new tab.",
        icon: "fa6-solid:folder-open",
    },
    ActionDef {
        id: "saveDocument",
        display_name: "Save",
        description: "Save the current document. Re-saves to the same `.darkly` file after the first Save As; otherwise opens the Save picker (`.darkly`, or PNG / JPEG / WebP to export the canvas).",
        icon: "fa6-solid:floppy-disk",
    },
    ActionDef {
        id: "saveDocumentAs",
        display_name: "Save As",
        description: "Save the current document to a new file — `.darkly`, or PNG / JPEG / WebP to export the canvas.",
        icon: "fa6-solid:file-export",
    },
    ActionDef {
        id: "exportTimelapse",
        display_name: "Export Timelapse…",
        description: "Export the process recording as an MP4 or GIF timelapse.",
        icon: "fa6-solid:video",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "file",
        actions: ACTIONS,
    }
}
