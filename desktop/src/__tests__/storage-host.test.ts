/**
 * Tests the Node-side StorageHost against a temp directory.
 * Catches drift in the bridge contract from the host side: if this test
 * breaks, the public/private repos have gotten out of sync on the shape of
 * window.electronAPI.storage. The renderer-side companion test lives at
 * frontend/src/storage/__tests__/node.test.ts in the public repo.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import * as fs from 'fs/promises';
import * as path from 'path';
import * as os from 'os';
import { createStorageHost, type StorageHost } from '../storage-host';

describe('storage-host (Node fs implementation)', () => {
    let root: string;
    let host: StorageHost;

    beforeEach(async () => {
        root = await fs.mkdtemp(path.join(os.tmpdir(), 'darkly-test-'));
        host = createStorageHost(() => root);
    });

    afterEach(async () => {
        await fs.rm(root, { recursive: true, force: true });
    });

    it('round-trips bytes through write → read', async () => {
        const data = new Uint8Array([1, 2, 3, 4, 5]);
        await host.write('presets/foo.json', data);
        expect(await host.read('presets/foo.json')).toEqual(data);
    });

    it('creates parent directories on write', async () => {
        await host.write('a/b/c/d.txt', new Uint8Array([7]));
        expect(await host.exists('a/b/c/d.txt')).toBe(true);
        expect(await host.exists('a/b/c')).toBe(true);
    });

    it('returns null when reading a missing file', async () => {
        expect(await host.read('nope.bin')).toBeNull();
    });

    it('returns null when reading a directory (not a file)', async () => {
        await host.write('dir/x.txt', new Uint8Array([1]));
        expect(await host.read('dir')).toBeNull();
    });

    it('list returns entries with kind', async () => {
        await host.write('presets/krita.json', new Uint8Array([1]));
        await host.write('presets/gimp.json', new Uint8Array([2]));
        await host.write('brushes/round.brush', new Uint8Array([3]));

        const rootEnts = await host.list('');
        expect(rootEnts.map(e => e.name).sort()).toEqual(['brushes', 'presets']);
        expect(rootEnts.every(e => e.kind === 'directory')).toBe(true);

        const presetEnts = await host.list('presets');
        expect(presetEnts.map(e => e.name).sort()).toEqual(['gimp.json', 'krita.json']);
        expect(presetEnts.every(e => e.kind === 'file')).toBe(true);
    });

    it('list returns [] for a missing directory', async () => {
        expect(await host.list('missing')).toEqual([]);
    });

    it('remove is recursive', async () => {
        await host.write('presets/a.json', new Uint8Array([1]));
        await host.write('presets/b.json', new Uint8Array([2]));
        await host.write('presets/nested/c.json', new Uint8Array([3]));
        await host.remove('presets');
        expect(await host.exists('presets')).toBe(false);
    });

    it('remove is idempotent on missing paths', async () => {
        await expect(host.remove('never-existed.json')).resolves.toBeUndefined();
    });

    it('exists works for files and dirs and missing paths', async () => {
        await host.write('a/b/c.txt', new Uint8Array([1]));
        expect(await host.exists('a/b/c.txt')).toBe(true);
        expect(await host.exists('a/b')).toBe(true);
        expect(await host.exists('a')).toBe(true);
        expect(await host.exists('a/x')).toBe(false);
        expect(await host.exists('z/y')).toBe(false);
    });

    it('write overwrites existing content', async () => {
        await host.write('foo.bin', new Uint8Array([1, 2, 3]));
        await host.write('foo.bin', new Uint8Array([4, 5]));
        expect(await host.read('foo.bin')).toEqual(new Uint8Array([4, 5]));
    });

    it('rejects parent-escape paths', async () => {
        await expect(host.read('../escape.txt')).rejects.toThrow(/escapes root/);
        await expect(host.write('../bad.txt', new Uint8Array([1]))).rejects.toThrow(/escapes root/);
        await expect(host.remove('../bad.txt')).rejects.toThrow(/escapes root/);
        await expect(host.exists('../anything')).rejects.toThrow(/escapes root/);
        await expect(host.list('../parent')).rejects.toThrow(/escapes root/);
    });

    it('rejects absolute paths', async () => {
        await expect(host.read('/etc/passwd')).rejects.toThrow(/relative/);
        await expect(host.write('/tmp/bad.txt', new Uint8Array([1]))).rejects.toThrow(/relative/);
    });

    it('rejects paths with backslashes', async () => {
        await expect(host.read('a\\b.txt')).rejects.toThrow(/forward slashes/);
    });

    it('refuses to delete the storage root', async () => {
        await host.write('keep.txt', new Uint8Array([1]));
        await host.remove('');
        await host.remove('.');
        await host.remove('/');
        expect(await host.exists('keep.txt')).toBe(true);
    });
});
