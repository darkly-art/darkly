import type { Preset } from '../schema';

export const PRESET_PHOTOSHOP: Preset = {
    name: 'Photoshop',
    description: 'Adobe Photoshop-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyB',
            eraserTool: 'KeyE',
            fillTool: 'KeyG',
            gradientTool: 'Shift+KeyG',
            colorPickerTool: 'KeyI',
        },
    },
};
