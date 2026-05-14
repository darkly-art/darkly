/**
 * Tests the NodeFsStorage adapter against an in-memory mock of the host
 * bridge. Catches any drift in the bridge contract from the renderer side
 * — if this test breaks, the public/private repos have gotten out of sync
 * on the shape of `window.electronAPI.storage`.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { NodeFsStorage } from '../node';
import type { DirEntry, ElectronStorageBridge } from '../types';

/** In-memory bridge mock: a flat map of path → bytes. Directories are
 *  inferred from path prefixes (no explicit mkdir). */
function makeMockBridge(): ElectronStorageBridge {
    const files = new Map<string, Uint8Array>();

    function splitPath(p: string): { dir: string; name: string } {
        const idx = p.lastIndexOf('/');
        return idx < 0
            ? { dir: '', name: p }
            : { dir: p.slice(0, idx), name: p.slice(idx + 1) };
    }

    return {
        async read(path) {
            return files.get(path) ?? null;
        },
        async write(path, data) {
            files.set(path, data);
        },
        async list(dir) {
            const prefix = dir === '' ? '' : `${dir}/`;
            const seen = new Map<string, 'file' | 'directory'>();
            for (const p of files.keys()) {
                if (!p.startsWith(prefix)) continue;
                const rest = p.slice(prefix.length);
                const slash = rest.indexOf('/');
                if (slash < 0) {
                    seen.set(rest, 'file');
                } else {
                    const name = rest.slice(0, slash);
                    // Don't downgrade a file entry; first seen wins.
                    if (!seen.has(name)) seen.set(name, 'directory');
                }
            }
            const out: DirEntry[] = [];
            for (const [name, kind] of seen) out.push({ name, kind });
            return out;
        },
        async remove(path) {
            // Remove the file itself plus any descendants (recursive).
            const prefix = `${path}/`;
            files.delete(path);
            for (const p of [...files.keys()]) {
                if (p.startsWith(prefix)) files.delete(p);
            }
        },
        async exists(path) {
            if (files.has(path)) return true;
            const prefix = `${path}/`;
            for (const p of files.keys()) {
                if (p.startsWith(prefix)) return true;
            }
            return false;
        },
    };
}

describe('NodeFsStorage (bridge contract)', () => {
    let bridge: ElectronStorageBridge;
    let storage: NodeFsStorage;

    beforeEach(() => {
        bridge = makeMockBridge();
        storage = new NodeFsStorage(bridge);
    });

    it('round-trips bytes through write → read', async () => {
        const payload = new Uint8Array([1, 2, 3, 4, 5]);
        await storage.write('presets/My Settings.json', payload);
        const out = await storage.read('presets/My Settings.json');
        expect(out).toEqual(payload);
    });

    it('returns null when reading a missing file', async () => {
        expect(await storage.read('nonexistent.json')).toBeNull();
    });

    it('lists files and inferred subdirectories', async () => {
        await storage.write('presets/krita.json', new Uint8Array([1]));
        await storage.write('presets/gimp.json', new Uint8Array([2]));
        await storage.write('brushes/round.brush', new Uint8Array([3]));

        const root = await storage.list('');
        const rootNames = root.map(e => e.name).sort();
        expect(rootNames).toEqual(['brushes', 'presets']);
        expect(root.every(e => e.kind === 'directory')).toBe(true);

        const presets = await storage.list('presets');
        const presetNames = presets.map(e => e.name).sort();
        expect(presetNames).toEqual(['gimp.json', 'krita.json']);
        expect(presets.every(e => e.kind === 'file')).toBe(true);
    });

    it('remove is idempotent on missing paths', async () => {
        await expect(storage.remove('never-existed.json')).resolves.toBeUndefined();
    });

    it('remove deletes a single file', async () => {
        await storage.write('presets/foo.json', new Uint8Array([1]));
        await storage.remove('presets/foo.json');
        expect(await storage.read('presets/foo.json')).toBeNull();
        expect(await storage.exists('presets/foo.json')).toBe(false);
    });

    it('remove is recursive on directories', async () => {
        await storage.write('presets/a.json', new Uint8Array([1]));
        await storage.write('presets/b.json', new Uint8Array([2]));
        await storage.remove('presets');
        expect(await storage.exists('presets')).toBe(false);
        expect(await storage.list('presets')).toEqual([]);
    });

    it('exists distinguishes present and absent paths', async () => {
        await storage.write('a/b/c.txt', new Uint8Array([1]));
        expect(await storage.exists('a/b/c.txt')).toBe(true);
        expect(await storage.exists('a/b')).toBe(true);
        expect(await storage.exists('a')).toBe(true);
        expect(await storage.exists('a/x')).toBe(false);
    });

    it('write overwrites existing content', async () => {
        await storage.write('foo.bin', new Uint8Array([1, 2, 3]));
        await storage.write('foo.bin', new Uint8Array([4, 5]));
        expect(await storage.read('foo.bin')).toEqual(new Uint8Array([4, 5]));
    });
});
