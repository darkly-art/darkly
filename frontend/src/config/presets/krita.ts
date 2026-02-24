import type { Preset } from '../schema';

export const PRESET_KRITA: Preset = {
    name: 'Krita',
    description: 'Default Krita-style keybindings',
    overrides: {
        // Krita defaults match our USER_DEFAULTS, so overrides are minimal.
        // This preset exists so switching back from Photoshop/GIMP restores Krita bindings.
        hotkeys: {
            brushTool: 'KeyB',
            eraserTool: 'KeyE',
            fillTool: 'KeyF',
            gradientTool: 'KeyG',
            colorPickerTool: 'KeyP',
        },
    },
};
