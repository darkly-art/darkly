import { describe, it, expect, vi, beforeEach } from 'vitest';

// Regression: the color-picker cursor used to hardcode Ctrl/Meta as the
// arming modifier, so swapping to the Photoshop preset (which binds
// `sampleColor` to `alt+drag`) left the cursor armed on Ctrl and unresponsive
// to Alt. Engagement now derives from whatever `sampleColor` is bound to.

const { fakeApp, fakeConfig } = vi.hoisted(() => {
    const fakeApp = {
        handle: null,
        activeLayerId: null as number | null,
        canvasEl: null as HTMLCanvasElement | null,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        background: { r: 255, g: 255, b: 255, a: 255 },
    };
    const fakeConfig = {
        _binding: 'canvas@paint:ctrl+drag' as string | null,
        get: vi.fn((key: string) => {
            if (key === 'mouseclicks.sampleColor') return fakeConfig._binding;
            return undefined;
        }),
        onChange: vi.fn(() => () => undefined),
    };
    return { fakeApp, fakeConfig };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));
// SVG asset import isn't relevant here; stub it so Vite's `?raw` suffix
// doesn't blow up under Vitest.
vi.mock('../../assets/color-picker.svg?raw', () => ({
    default: '<svg><path d="M0,0 L1,1"/></svg>',
}));

import { modPrefixOfChord, colorPickerEngagementMods } from '../colorpicker_cursor';

beforeEach(() => {
    fakeConfig._binding = 'canvas@paint:ctrl+drag';
    fakeConfig.get.mockClear();
});

describe('modPrefixOfChord', () => {
    it('extracts the modifier prefix from a drag chord', () => {
        expect(modPrefixOfChord('ctrl+drag')).toBe('ctrl');
        expect(modPrefixOfChord('alt+drag')).toBe('alt');
        expect(modPrefixOfChord('shift+rightDrag')).toBe('shift');
        expect(modPrefixOfChord('ctrl+alt+middleDrag')).toBe('ctrl+alt');
    });

    it('canonicalises modifier order (ctrl, alt, shift)', () => {
        expect(modPrefixOfChord('alt+ctrl+drag')).toBe('ctrl+alt');
        expect(modPrefixOfChord('shift+ctrl+drag')).toBe('ctrl+shift');
    });

    it('returns null for non-drag chords', () => {
        expect(modPrefixOfChord('ctrl+click')).toBeNull();
        expect(modPrefixOfChord('click')).toBeNull();
        expect(modPrefixOfChord('alt+doubleClick')).toBeNull();
    });

    it('returns "" for a bare drag (no modifier)', () => {
        expect(modPrefixOfChord('drag')).toBe('');
    });
});

describe('colorPickerEngagementMods (sampleColor binding → arm set)', () => {
    it('follows the Krita/GIMP ctrl+drag binding', () => {
        fakeConfig._binding = 'canvas@paint:ctrl+drag';
        const mods = colorPickerEngagementMods();
        expect(mods.has('ctrl')).toBe(true);
        expect(mods.has('alt')).toBe(false);
    });

    it('follows the Photoshop alt+drag binding (the original bug)', () => {
        fakeConfig._binding = 'canvas@paint:alt+drag';
        const mods = colorPickerEngagementMods();
        expect(mods.has('alt')).toBe(true);
        expect(mods.has('ctrl')).toBe(false);
    });

    it('honors multi-binding strings (joined with |)', () => {
        fakeConfig._binding = 'canvas@paint:ctrl+drag|canvas@paint:alt+drag';
        const mods = colorPickerEngagementMods();
        expect(mods.has('ctrl')).toBe(true);
        expect(mods.has('alt')).toBe(true);
    });

    it('ignores bindings whose site is not canvas', () => {
        fakeConfig._binding = 'layerPanel@paint:alt+drag';
        const mods = colorPickerEngagementMods();
        expect(mods.size).toBe(0);
    });

    it('ignores bindings whose scope is non-paint', () => {
        fakeConfig._binding = 'canvas@select:ctrl+drag';
        const mods = colorPickerEngagementMods();
        expect(mods.size).toBe(0);
    });

    it('ignores click/doubleClick chords (no pre-press arming phase)', () => {
        fakeConfig._binding = 'canvas@paint:ctrl+click';
        const mods = colorPickerEngagementMods();
        expect(mods.size).toBe(0);
    });

    it('ignores bare-drag bindings (no modifier ⇒ would fight every stroke)', () => {
        fakeConfig._binding = 'canvas@paint:drag';
        const mods = colorPickerEngagementMods();
        expect(mods.size).toBe(0);
    });

    it('returns the empty set when the action has no binding', () => {
        fakeConfig._binding = '';
        const mods = colorPickerEngagementMods();
        expect(mods.size).toBe(0);
    });
});
