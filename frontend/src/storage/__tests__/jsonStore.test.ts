import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { DarklyStorage, DirEntry } from '../types';
import { jsonFile, jsonDir } from '../jsonStore';

/** In-memory DarklyStorage, counting writes so coalescing is observable. */
class FakeStorage implements DarklyStorage {
    files = new Map<string, Uint8Array>();
    writes: string[] = [];
    /** Resolves each write after a tick, so overlapping writes can interleave
     *  if the lock does not hold them apart. */
    slow = false;

    async read(path: string) { return this.files.get(path) ?? null; }
    async write(path: string, data: Uint8Array) {
        if (this.slow) await new Promise(r => setTimeout(r, 5));
        this.files.set(path, data);
        this.writes.push(path);
    }
    async list(dir: string): Promise<DirEntry[]> {
        const prefix = dir ? `${dir}/` : '';
        const out: DirEntry[] = [];
        for (const p of this.files.keys()) {
            if (!p.startsWith(prefix)) continue;
            const rest = p.slice(prefix.length);
            if (rest.length === 0 || rest.includes('/')) continue;
            out.push({ name: rest, kind: 'file' });
        }
        return out;
    }
    async remove(path: string) { this.files.delete(path); }
    async exists(path: string) { return this.files.has(path); }

    json(path: string): unknown {
        const b = this.files.get(path);
        return b ? JSON.parse(new TextDecoder().decode(b)) : null;
    }
    put(path: string, text: string) {
        this.files.set(path, new TextEncoder().encode(text));
    }
}

describe('jsonFile', () => {
    let s: FakeStorage;
    beforeEach(() => { s = new FakeStorage(); });

    it('a_burst_of_writes_coalesces_into_one', async () => {
        const f = jsonFile<{ n: number }>('t.json', () => ({ n: 0 }), undefined, s);
        f.write({ n: 1 });
        f.write({ n: 2 });
        f.write({ n: 3 });
        await f.flush();

        expect(s.writes).toEqual(['t.json']);
        expect(s.json('t.json')).toEqual({ n: 3 });
    });

    it('writes_do_not_interleave', async () => {
        s.slow = true;
        const f = jsonFile<{ n: number }>('t.json', () => ({ n: 0 }), undefined, s);

        f.write({ n: 1 });
        const first = f.flush();
        f.write({ n: 2 });
        const second = f.flush();
        await Promise.all([first, second]);

        // Both landed, in issue order, so the later value is what survives.
        expect(s.writes).toEqual(['t.json', 't.json']);
        expect(s.json('t.json')).toEqual({ n: 2 });
    });

    it('a_missing_file_reads_as_the_fallback', async () => {
        const f = jsonFile<{ n: number }>('gone.json', () => ({ n: 42 }), undefined, s);
        await expect(f.read()).resolves.toEqual({ n: 42 });
    });

    it('malformed_json_reads_as_the_fallback', async () => {
        s.put('t.json', 'not json at all');
        const f = jsonFile<{ n: number }>('t.json', () => ({ n: 7 }), undefined, s);
        await expect(f.read()).resolves.toEqual({ n: 7 });
    });

    it('a_value_the_validator_rejects_reads_as_the_fallback', async () => {
        s.put('t.json', '{"n":"nope"}');
        const f = jsonFile<{ n: number }>(
            't.json',
            () => ({ n: 7 }),
            raw => {
                const o = raw as { n?: unknown };
                return typeof o.n === 'number' ? { n: o.n } : null;
            },
            s,
        );
        await expect(f.read()).resolves.toEqual({ n: 7 });
    });
});

describe('jsonDir', () => {
    let s: FakeStorage;
    beforeEach(() => { s = new FakeStorage(); });

    it('readAll_skips_a_record_that_fails_to_parse', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        s.put('packs/a.json', '{"name":"A"}');
        s.put('packs/b.json', 'corrupt{{{');
        s.put('packs/c.json', '{"name":"C"}');

        const d = jsonDir<{ name: string }>('packs', undefined, s);
        const all = await d.readAll();

        expect([...all.keys()].sort()).toEqual(['a', 'c']);
        expect(all.get('a')).toEqual({ name: 'A' });
        warn.mockRestore();
    });

    it('remove_deletes_exactly_one_record', async () => {
        s.put('packs/a.json', '{"name":"A"}');
        s.put('packs/b.json', '{"name":"B"}');

        const d = jsonDir<{ name: string }>('packs', undefined, s);
        await d.remove('a');

        const all = await d.readAll();
        expect([...all.keys()]).toEqual(['b']);
    });

    it('remove_cancels_a_queued_write_so_the_file_stays_gone', async () => {
        const d = jsonDir<{ name: string }>('packs', undefined, s);
        d.write('a', { name: 'A' });
        await d.remove('a');
        await d.flush();

        expect(await d.readAll()).toEqual(new Map());
    });

    it('writes_land_under_the_id_as_filename', async () => {
        const d = jsonDir<{ name: string }>('packs', undefined, s);
        d.write('9f1c', { name: 'Watercolors' });
        await d.flush();

        expect(s.json('packs/9f1c.json')).toEqual({ name: 'Watercolors' });
    });

    it('ids_that_sanitize_alike_stay_distinct_records', async () => {
        // Ids are opaque and filename-safe by construction, so two packs whose
        // *names* would collapse to one slug still get their own file.
        const d = jsonDir<{ name: string }>('packs', undefined, s);
        d.write('id-one', { name: 'A/B' });
        d.write('id-two', { name: 'A:B' });
        await d.flush();

        const all = await d.readAll();
        expect(all.size).toBe(2);
        expect(all.get('id-one')).toEqual({ name: 'A/B' });
        expect(all.get('id-two')).toEqual({ name: 'A:B' });
    });
});
