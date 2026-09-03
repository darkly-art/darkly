import { describe, it, expect, vi } from 'vitest';
// Installs a working `indexedDB` global so the library's persistence path runs
// for real under node.
import 'fake-indexeddb/auto';

// Fake multi-tab shell exposing one live engine handle, the replay target.
const { engine, instances } = vi.hoisted(() => {
    const engine = { send: vi.fn(() => Promise.resolve({ families: ['My Font'] })) };
    const instances = [{ engine }];
    return { engine, instances };
});
vi.mock('../../multi_tab/shell.svelte', () => ({ shell: { instances } }));

import { fontLibrary, contentHash } from '../font_library.svelte';
import { withApi } from '../../engine/testApi';

// The library reaches each handle through its typed api; layer one over the
// fake's `send` spy so `registerFont(bytes)` forwards to it.
withApi(engine);

/** Read every persisted record straight from IndexedDB. */
function readStore(): Promise<any[]> {
    return new Promise((resolve, reject) => {
        const req = indexedDB.open('darkly-fonts');
        req.onsuccess = () => {
            const db = req.result;
            const all = db.transaction('fonts', 'readonly').objectStore('fonts').getAll();
            all.onsuccess = () => {
                resolve(all.result);
                db.close();
            };
            all.onerror = () => reject(all.error);
        };
        req.onerror = () => reject(req.error);
    });
}

describe('fontLibrary', () => {
    it('registers into live handles, persists, dedups, and replays', async () => {
        const bytes = new Uint8Array([1, 2, 3, 4]);

        // add → registers into the live handle and returns its families.
        const families = await fontLibrary.add(bytes, 'upload');
        expect(families).toEqual(['My Font']);
        expect(engine.send).toHaveBeenCalledWith('register_font', {}, bytes);
        expect(fontLibrary.families).toContain('My Font');

        // Persisted under the content hash, keyed with its source.
        const stored = await readStore();
        const rec = stored.find((r) => r.hash === contentHash(bytes));
        expect(rec).toBeTruthy();
        expect(rec.source).toBe('upload');
        expect(rec.families).toEqual(['My Font']);

        // Re-adding identical bytes is a no-op beyond returning the families:
        // the content hash dedups, so no second registration fires.
        engine.send.mockClear();
        const again = await fontLibrary.add(bytes, 'upload');
        expect(again).toEqual(['My Font']);
        expect(engine.send).not.toHaveBeenCalled();

        // A freshly-created handle gets the whole library replayed into it.
        const fresh = withApi({ send: vi.fn(() => Promise.resolve({})) });
        await fontLibrary.registerIntoHandle(fresh as any);
        expect(fresh.send).toHaveBeenCalledWith('register_font', {}, bytes);
    });

    it('hashes identical bytes identically and differing bytes differently', () => {
        expect(contentHash(new Uint8Array([1, 2, 3]))).toBe(contentHash(new Uint8Array([1, 2, 3])));
        expect(contentHash(new Uint8Array([1, 2, 3]))).not.toBe(
            contentHash(new Uint8Array([1, 2, 4])),
        );
    });
});
