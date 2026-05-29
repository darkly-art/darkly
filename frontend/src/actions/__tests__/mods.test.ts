import { describe, it, expect, vi, beforeEach } from 'vitest';

// `mods.ts` caches `IS_MAC` at module load, so we have to re-import after
// re-mocking `detectPlatform` to exercise both branches.

beforeEach(() => {
    vi.resetModules();
    vi.unmock('../../platform');
});

async function loadModsAs(os: 'linux' | 'windows' | 'macos') {
    vi.doMock('../../platform', () => ({
        detectPlatform: () => ({ os, browser: 'chromium' }),
    }));
    return await import('../mods');
}

describe('canonicalModsFromEvent', () => {
    it('emits ctrl and meta as separate primitives — no fold', async () => {
        const { canonicalModsFromEvent } = await loadModsAs('linux');
        const e = { ctrlKey: true, metaKey: true, altKey: false, shiftKey: false };
        expect(canonicalModsFromEvent(e)).toEqual(['ctrl', 'meta']);
    });

    it('preserves canonical order regardless of input', async () => {
        const { canonicalModsFromEvent } = await loadModsAs('linux');
        const e = { ctrlKey: true, metaKey: false, altKey: true, shiftKey: true };
        expect(canonicalModsFromEvent(e)).toEqual(['ctrl', 'alt', 'shift']);
    });

    it('returns the empty list when nothing is held', async () => {
        const { canonicalModsFromEvent } = await loadModsAs('linux');
        const e = { ctrlKey: false, metaKey: false, altKey: false, shiftKey: false };
        expect(canonicalModsFromEvent(e)).toEqual([]);
    });
});

describe('substituteMod', () => {
    it('substitutes $mod → ctrl on Linux', async () => {
        const { substituteMod } = await loadModsAs('linux');
        expect(substituteMod('$mod+drag')).toBe('ctrl+drag');
        expect(substituteMod('$mod+Shift+KeyZ')).toBe('ctrl+Shift+KeyZ');
    });

    it('substitutes $mod → ctrl on Windows', async () => {
        const { substituteMod } = await loadModsAs('windows');
        expect(substituteMod('$mod+drag')).toBe('ctrl+drag');
    });

    it('substitutes $mod → meta on macOS', async () => {
        const { substituteMod } = await loadModsAs('macos');
        expect(substituteMod('$mod+drag')).toBe('meta+drag');
        expect(substituteMod('$mod+Shift+KeyZ')).toBe('meta+Shift+KeyZ');
    });

    it('leaves chords without $mod untouched', async () => {
        const { substituteMod } = await loadModsAs('linux');
        expect(substituteMod('alt+drag')).toBe('alt+drag');
        expect(substituteMod('shift+rightDrag')).toBe('shift+rightDrag');
        expect(substituteMod('click')).toBe('click');
    });
});

describe('substituteModInBinding', () => {
    it('substitutes only inside the chord portion of a sited binding', async () => {
        const { substituteModInBinding } = await loadModsAs('linux');
        expect(substituteModInBinding('canvas@paint:$mod+drag')).toBe('canvas@paint:ctrl+drag');
        expect(substituteModInBinding('layerPanel:$mod+click')).toBe('layerPanel:ctrl+click');
    });

    it('handles a bare (no site/scope) binding', async () => {
        const { substituteModInBinding } = await loadModsAs('macos');
        expect(substituteModInBinding('$mod+KeyZ')).toBe('meta+KeyZ');
    });

    it('leaves site/scope strings alone even if they contain "$mod" (defensive)', async () => {
        const { substituteModInBinding } = await loadModsAs('linux');
        // Hypothetical malformed binding — the site/scope grammar doesn't
        // permit `$mod`, but if it did, only the chord portion should be
        // substituted.
        expect(substituteModInBinding('$mod:$mod+drag')).toBe('$mod:ctrl+drag');
    });
});

describe('MOD_KEY', () => {
    it('is ctrl on Linux', async () => {
        const { MOD_KEY } = await loadModsAs('linux');
        expect(MOD_KEY).toBe('ctrl');
    });

    it('is meta on macOS', async () => {
        const { MOD_KEY } = await loadModsAs('macos');
        expect(MOD_KEY).toBe('meta');
    });
});

describe('isModEvent', () => {
    it('returns ctrlKey on Linux/Win and ignores metaKey', async () => {
        const { isModEvent } = await loadModsAs('linux');
        expect(isModEvent({ ctrlKey: true,  metaKey: false })).toBe(true);
        expect(isModEvent({ ctrlKey: false, metaKey: true  })).toBe(false);
        expect(isModEvent({ ctrlKey: false, metaKey: false })).toBe(false);
    });

    it('returns metaKey on macOS and ignores ctrlKey', async () => {
        const { isModEvent } = await loadModsAs('macos');
        expect(isModEvent({ ctrlKey: false, metaKey: true  })).toBe(true);
        expect(isModEvent({ ctrlKey: true,  metaKey: false })).toBe(false);
        expect(isModEvent({ ctrlKey: false, metaKey: false })).toBe(false);
    });
});
