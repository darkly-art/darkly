import { describe, it, expect, beforeEach } from 'vitest';
import type { DarklyStorage, DirEntry } from '../../storage/types';
import { createRecents } from '../recents.svelte';

class FakeStorage implements DarklyStorage {
    files = new Map<string, Uint8Array>();
    writes = 0;

    async read(path: string) { return this.files.get(path) ?? null; }
    async write(path: string, data: Uint8Array) { this.files.set(path, data); this.writes++; }
    async list(): Promise<DirEntry[]> { return []; }
    async remove(path: string) { this.files.delete(path); }
    async exists(path: string) { return this.files.has(path); }

    json(): { brushes?: string[]; colors?: string[] } | null {
        const b = this.files.get('recents.json');
        return b ? JSON.parse(new TextDecoder().decode(b)) : null;
    }
    put(text: string) {
        this.files.set('recents.json', new TextEncoder().encode(text));
    }
}

describe('recents', () => {
    let s: FakeStorage;
    beforeEach(() => { s = new FakeStorage(); });

    it('push_moves_an_existing_entry_to_the_front', async () => {
        const r = createRecents(s);
        r.brushes.use('a');
        r.brushes.use('b');
        r.brushes.use('c');
        r.brushes.use('a');

        expect(r.brushes.items).toEqual(['a', 'c', 'b']);
    });

    it('push_evicts_the_oldest_beyond_the_cap', async () => {
        const r = createRecents(s);
        // Cap is 12; push 15 distinct brushes.
        for (let i = 0; i < 15; i++) r.brushes.use(`b${i}`);

        expect(r.brushes.items).toHaveLength(12);
        expect(r.brushes.items[0]).toBe('b14');
        expect(r.brushes.items).not.toContain('b0');
        expect(r.brushes.items).not.toContain('b2');
    });

    it('pushing_the_current_front_writes_nothing', async () => {
        const r = createRecents(s);
        r.brushes.use('a');
        await r.flush();
        const after = s.writes;

        r.brushes.use('a');
        r.brushes.use('a');
        await r.flush();

        expect(s.writes).toBe(after);
    });

    it('a_malformed_stored_value_reads_as_empty', async () => {
        for (const stored of ['not json', '{"a":1}', '{"brushes":5}', '[]']) {
            const fake = new FakeStorage();
            fake.put(stored);
            const r = createRecents(fake);
            await r.load();
            expect(r.brushes.items, `for ${stored}`).toEqual([]);
            expect(r.colors.items, `for ${stored}`).toEqual([]);
        }
    });

    it('non_string_members_are_dropped_on_read', async () => {
        s.put('{"brushes":["ok",5,null,"also"],"colors":[]}');
        const r = createRecents(s);
        await r.load();
        expect(r.brushes.items).toEqual(['ok', 'also']);
    });

    it('colors_dedupe_on_rgb_ignoring_alpha', async () => {
        const r = createRecents(s);
        r.colors.use('#ff0000ff');
        r.colors.use('#00ff00ff');
        r.colors.use('#ff000080');

        // One red entry, carrying the alpha it was last used at.
        expect(r.colors.items).toEqual(['#ff000080', '#00ff00ff']);
    });

    it('both_lists_share_one_file', async () => {
        const r = createRecents(s);
        r.brushes.use('ink_pen');
        r.colors.use('#3355ffff');
        await r.flush();

        expect(s.json()).toEqual({ brushes: ['ink_pen'], colors: ['#3355ffff'] });
    });

    it('a_stored_list_survives_a_reload', async () => {
        const first = createRecents(s);
        first.brushes.use('ink_pen');
        first.colors.use('#3355ffff');
        await first.flush();

        const second = createRecents(s);
        await second.load();
        expect(second.brushes.items).toEqual(['ink_pen']);
        expect(second.colors.items).toEqual(['#3355ffff']);
    });

    it('retain_drops_entries_that_no_longer_resolve', async () => {
        const r = createRecents(s);
        r.brushes.use('gone');
        r.brushes.use('kept');
        await r.flush();

        r.brushes.retain(id => id !== 'gone');
        await r.flush();

        expect(r.brushes.items).toEqual(['kept']);
        expect(s.json()?.brushes).toEqual(['kept']);
    });

    it('retain_that_drops_nothing_writes_nothing', async () => {
        const r = createRecents(s);
        r.brushes.use('kept');
        await r.flush();
        const after = s.writes;

        r.brushes.retain(() => true);
        await r.flush();

        expect(s.writes).toBe(after);
    });
});
