import { describe, it, expect, beforeEach } from 'vitest';
import { zipSync, strToU8 } from 'fflate';
import type { DarklyStorage, DirEntry } from '../types';
import {
    writeSnapshot,
    readSnapshot,
    removeSnapshot,
    listSnapshots,
    snapshotDocName,
    snapshotThumbnail,
} from '../recovery';

/** In-memory DarklyStorage for node tests. */
class FakeStorage implements DarklyStorage {
    files = new Map<string, Uint8Array>();
    async read(path: string) { return this.files.get(path) ?? null; }
    async write(path: string, data: Uint8Array) { this.files.set(path, data); }
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
}

/** A minimal valid `.darkly` zip carrying a manifest name + thumbnail. */
function fakeDarkly(name: string, withThumb = true): Uint8Array {
    const entries: Record<string, Uint8Array> = {
        'manifest.json': strToU8(JSON.stringify({ name })),
    };
    if (withThumb) entries['thumbnail.png'] = new Uint8Array([1, 2, 3, 4]);
    return zipSync(entries);
}

const SID = 'sess-1';
const RID = 'tab-1';

describe('recovery store', () => {
    let s: FakeStorage;
    beforeEach(() => { s = new FakeStorage(); });

    it('round-trips a snapshot: write → read → remove', async () => {
        const bytes = fakeDarkly('My Drawing');
        await writeSnapshot(SID, RID, bytes, s);

        const got = await readSnapshot(SID, RID, s);
        expect(got).toEqual(bytes);

        await removeSnapshot(SID, RID, s);
        expect(await readSnapshot(SID, RID, s)).toBeNull();
    });

    it('overwrites the same file on repeated writes (latest-only)', async () => {
        await writeSnapshot(SID, RID, fakeDarkly('v1'), s);
        await writeSnapshot(SID, RID, fakeDarkly('v2'), s);
        const entries = await listSnapshots(s);
        expect(entries).toHaveLength(1);
        expect(entries[0].name).toBe('v2');
    });

    it('lists snapshots with ids parsed from the filename and name from the zip', async () => {
        await writeSnapshot('sA', 'rA', fakeDarkly('Alpha'), s);
        await writeSnapshot('sB', 'rB', fakeDarkly('Beta'), s);

        const entries = await listSnapshots(s);
        const byId = Object.fromEntries(entries.map((e) => [e.recoveryId, e]));
        expect(byId['rA']).toMatchObject({ sessionId: 'sA', name: 'Alpha' });
        expect(byId['rB']).toMatchObject({ sessionId: 'sB', name: 'Beta' });
    });

    it('skips a corrupt zip (no manifest) during listing', async () => {
        // Write a snapshot-named file whose contents are not a valid zip.
        await s.write('recovery/sess-1~broken.darkly', new Uint8Array([0, 1, 2, 3]));
        await writeSnapshot('sess-1', 'good', fakeDarkly('Good'), s);

        const entries = await listSnapshots(s);
        expect(entries).toHaveLength(1);
        expect(entries[0].recoveryId).toBe('good');
    });

    it('reads document name and thumbnail out of a snapshot zip', () => {
        const withThumb = fakeDarkly('Named', true);
        expect(snapshotDocName(withThumb)).toBe('Named');
        expect(snapshotThumbnail(withThumb)).toEqual(new Uint8Array([1, 2, 3, 4]));

        const noThumb = fakeDarkly('NoThumb', false);
        expect(snapshotThumbnail(noThumb)).toBeNull();
    });

    it('falls back to a default name when the manifest is unreadable', () => {
        expect(snapshotDocName(new Uint8Array([9, 9, 9]))).toBe('Recovered document');
    });
});
