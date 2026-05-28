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
    it('follows a literal ctrl+drag binding', () => {
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

// `$mod` resolution is platform-dependent and `mods.ts` caches the platform
// at module load, so each case re-mocks `platform.ts` and re-imports the
// module under test. The other mocks (app/config/svg) remain registered
// from the top-of-file `vi.mock` calls and re-apply after `resetModules`.
describe('colorPickerEngagementMods — $mod resolution per platform', () => {
    async function loadAs(os: 'linux' | 'windows' | 'macos') {
        vi.resetModules();
        vi.doMock('../../platform', () => ({
            detectPlatform: () => ({ os, browser: 'chromium' }),
        }));
        return await import('../colorpicker_cursor');
    }

    it('Krita/GIMP $mod+drag binding arms on ctrl (not meta) on Linux', async () => {
        fakeConfig._binding = 'canvas@paint:$mod+drag';
        const mod = await loadAs('linux');
        const mods = mod.colorPickerEngagementMods();
        expect(mods.has('ctrl')).toBe(true);
        expect(mods.has('meta')).toBe(false);
    });

    it('Krita/GIMP $mod+drag binding arms on ctrl (not meta) on Windows', async () => {
        fakeConfig._binding = 'canvas@paint:$mod+drag';
        const mod = await loadAs('windows');
        const mods = mod.colorPickerEngagementMods();
        expect(mods.has('ctrl')).toBe(true);
        expect(mods.has('meta')).toBe(false);
    });

    it('Krita/GIMP $mod+drag binding arms on meta (Cmd, not ctrl) on macOS', async () => {
        fakeConfig._binding = 'canvas@paint:$mod+drag';
        const mod = await loadAs('macos');
        const mods = mod.colorPickerEngagementMods();
        expect(mods.has('meta')).toBe(true);
        expect(mods.has('ctrl')).toBe(false);
    });

    it('regression: Photoshop alt+drag arms on alt on every platform', async () => {
        fakeConfig._binding = 'canvas@paint:alt+drag';
        for (const os of ['linux', 'windows', 'macos'] as const) {
            const mod = await loadAs(os);
            const mods = mod.colorPickerEngagementMods();
            expect(mods.has('alt')).toBe(true);
            expect(mods.has('ctrl')).toBe(false);
            expect(mods.has('meta')).toBe(false);
        }
    });

    it('regression: a literal ctrl binding does NOT match meta (Super key) on Linux', async () => {
        // The original bug: the metaKey→ctrl fold meant holding the
        // Windows/Super key on Linux phantom-armed the picker on any
        // ctrl-based binding. With the fold gone, the engagement set is
        // {'ctrl'} — and `currentMods` derived from canonicalModsFromEvent
        // reports `'meta'` (not `'ctrl'`) when only Super is held, so
        // engagement check fails. We assert the engagement set directly.
        fakeConfig._binding = 'canvas@paint:ctrl+drag';
        const mod = await loadAs('linux');
        const mods = mod.colorPickerEngagementMods();
        expect(mods.has('ctrl')).toBe(true);
        expect(mods.has('meta')).toBe(false);
    });
});
