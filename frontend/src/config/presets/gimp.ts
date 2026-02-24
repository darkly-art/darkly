import type { Preset } from '../schema';

export const PRESET_GIMP: Preset = {
    name: 'GIMP',
    description: 'GIMP-style keybindings',
    overrides: {
        hotkeys: {
            brushTool: 'KeyP',
            eraserTool: 'Shift+KeyE',
            fillTool: 'Shift+KeyB',
            gradientTool: 'KeyG',
            colorPickerTool: 'KeyO',
        },
    },
};
