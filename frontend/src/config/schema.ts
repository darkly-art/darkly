// --- Tool hotkey imports (each tool defines its own default hotkey) ---
import { BRUSH_HOTKEY } from '../tools/brush.svelte';
import { ERASER_HOTKEY } from '../tools/eraser.svelte';
import { FILL_HOTKEY } from '../tools/fill.svelte';
import { GRADIENT_HOTKEY } from '../tools/gradient.svelte';
import { COLORPICKER_HOTKEY } from '../tools/colorpicker.svelte';

// --- Project config (saved per document) ---

export interface ProjectConfig {
    canvas: {
        width: number;
        height: number;
        backgroundColor: string;
    };
}

export const PROJECT_DEFAULTS: ProjectConfig = {
    canvas: {
        width: 1920,
        height: 1080,
        backgroundColor: '#1a1a1a',
    },
};

// --- User config (global, persists across documents) ---

export interface HotkeyMap {
    // Canvas navigation (modifier+drag combos handled by navigation state machine,
    // not tinykeys -- but listed here for preset customization)
    panModifier: string;
    rotateModifier: string;
    zoomModifier: string;

    // Color
    resetColors: string;
    swapColors: string;

    // Edit
    undo: string;
    redo: string;

    // Tools -- default values sourced from each tool module
    brushTool: string;
    eraserTool: string;
    fillTool: string;
    gradientTool: string;
    colorPickerTool: string;

    // Brush size / opacity
    brushSizeUp: string;
    brushSizeDown: string;
    opacityUp: string;
    opacityDown: string;
}

export interface UserConfig {
    colors: {
        defaultForeground: string;
        defaultBackground: string;
    };
    ui: {
        leftSidebarWidth: number;
        rightSidebarWidth: number;
    };
    hotkeys: HotkeyMap;
}

export type DeepPartial<T> = {
    [P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P];
};

export interface Preset {
    name: string;
    description: string;
    overrides: DeepPartial<UserConfig>;
}

export const USER_DEFAULTS: UserConfig = {
    colors: {
        defaultForeground: '#000000',
        defaultBackground: '#ffffff',
    },
    ui: {
        leftSidebarWidth: 48,
        rightSidebarWidth: 260,
    },
    hotkeys: {
        panModifier: 'Space',
        rotateModifier: 'Shift+Space',
        zoomModifier: 'Ctrl+Space',
        resetColors: 'KeyD',
        swapColors: 'KeyX',
        undo: '$mod+KeyZ',
        redo: '$mod+Shift+KeyZ',
        // Tool hotkeys -- sourced from each tool module
        brushTool: BRUSH_HOTKEY,
        eraserTool: ERASER_HOTKEY,
        fillTool: FILL_HOTKEY,
        gradientTool: GRADIENT_HOTKEY,
        colorPickerTool: COLORPICKER_HOTKEY,
        brushSizeUp: 'BracketRight',
        brushSizeDown: 'BracketLeft',
        opacityUp: 'KeyO',
        opacityDown: 'KeyI',
    },
};
