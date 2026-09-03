use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "openSettings",
        display_name: "Settings",
        description: "Show the preferences modal.",
        icon: "fa6-solid:gear",
    },
    ActionDef {
        id: "commandPalette",
        display_name: "Command Palette",
        description: "Search and run any command.",
        icon: "fa6-solid:magnifying-glass",
    },
    ActionDef {
        id: "mirrorViewH",
        display_name: "Mirror View",
        description: "Flip the canvas horizontally for fresh-eyes review. View-only: the document is unchanged.",
        icon: "fa6-solid:left-right",
    },
    ActionDef {
        id: "resetView",
        display_name: "Reset View",
        description: "Reset rotation, mirror, pan, and zoom-to-fit. View-only: the document is unchanged.",
        icon: "fa6-solid:expand",
    },
    ActionDef {
        id: "fitToScreen",
        display_name: "Fit to Screen",
        description: "Zoom and recenter so the whole canvas fills the viewport, keeping the current rotation and mirror. View-only: the document is unchanged.",
        icon: "fa6-solid:maximize",
    },
    ActionDef {
        id: "centerView",
        display_name: "Center View",
        description: "Recenter the canvas in the viewport without changing zoom, rotation, or mirror. View-only: the document is unchanged.",
        icon: "fa6-solid:crosshairs",
    },
    ActionDef {
        id: "openCheatsheet",
        display_name: "Hotkey Cheat Sheet",
        description: "Open a searchable, printable list of every keyboard shortcut.",
        icon: "fa6-solid:keyboard",
    },
    ActionDef {
        id: "openDocs",
        display_name: "Documentation",
        description: "Open the Darkly documentation in a new tab.",
        icon: "fa6-solid:book",
    },
    ActionDef {
        id: "openWebsite",
        display_name: "Website",
        description: "Open the Darkly website in a new tab.",
        icon: "fa6-solid:globe",
    },
    ActionDef {
        id: "openGithub",
        display_name: "GitHub Repository",
        description: "Open the Darkly source repository on GitHub.",
        icon: "fa6-brands:github",
    },
    ActionDef {
        id: "aboutDarkly",
        display_name: "About Darkly",
        description: "Show version and credits.",
        icon: "fa6-solid:circle-info",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "view",
        actions: ACTIONS,
    }
}
