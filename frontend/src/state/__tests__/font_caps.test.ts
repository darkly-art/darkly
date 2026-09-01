import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Control the invalidation token directly: a plain object standing in for the
// library store, so a test can swap `families` to a fresh array to simulate a
// re-import without touching IndexedDB.
const { lib } = vi.hoisted(() => ({ lib: { families: [] as string[] } }));
vi.mock('../font_library.svelte', () => ({ fontLibrary: lib }));

import { fontCaps } from '../font_caps.svelte';

const CAPS = { italic: true, axes: [{ tag: 'wght', min: 100, default: 400, max: 900 }] };

function fakeEngine(caps: unknown = CAPS) {
    return withApi({ send: vi.fn(() => Promise.resolve(caps)) });
}

beforeEach(() => {
    // A fresh array identity invalidates the module-level cache between tests.
    lib.families = [];
});

describe('font capabilities cache', () => {
    it('normalizes a missing response to empty capabilities', async () => {
        const engine = fakeEngine(null);
        const caps = await fontCaps(engine as never, 'Ghost Font');
        expect(caps).toEqual({ italic: false, axes: [] });
    });

    it('caches per family, so a repeat lookup does not refetch', async () => {
        const engine = fakeEngine();
        const first = await fontCaps(engine as never, 'Noto Sans');
        const second = await fontCaps(engine as never, 'Noto Sans');
        expect(first).toEqual(CAPS);
        expect(second).toEqual(CAPS);
        expect(engine.send).toHaveBeenCalledTimes(1);
    });

    it('invalidates the cache when the font library changes (re-import)', async () => {
        const engine = fakeEngine();
        await fontCaps(engine as never, 'Roboto');
        expect(engine.send).toHaveBeenCalledTimes(1);
        // A re-import replaces the families array; the same-named family may now
        // have different axes, so the next lookup must refetch.
        lib.families = ['Roboto'];
        await fontCaps(engine as never, 'Roboto');
        expect(engine.send).toHaveBeenCalledTimes(2);
    });
});
