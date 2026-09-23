/**
 * Node-side filesystem implementation behind the storage IPC handlers.
 *
 * The renderer (Darkly frontend) talks to this via `window.electronAPI.storage`,
 * exposed by preload.ts. The contract is declared in the public repo at
 * frontend/src/storage/types.ts (interface `ElectronStorageBridge`). Any drift
 * between the two sides breaks the app: the regression test in
 * __tests__/storage-host.test.ts pins this side of the contract, and the
 * companion test in the public repo pins the renderer side.
 *
 * All paths are forward-slash, relative to the storage root (userData dir).
 * The host validates every path against the root before touching the disk.
 */
import * as fs from 'fs/promises';
import * as path from 'path';

export interface DirEntry {
    name: string;
    kind: 'file' | 'directory';
}

export interface StorageHost {
    read(p: string): Promise<Uint8Array | null>;
    write(p: string, data: Uint8Array): Promise<void>;
    list(dir: string): Promise<DirEntry[]>;
    remove(p: string): Promise<void>;
    exists(p: string): Promise<boolean>;
}

/** Resolve a renderer-supplied relative path against the storage root.
 *  Rejects anything that could escape the root or rely on platform-specific
 *  separator handling. Returns the absolute on-disk path. */
function safeJoin(root: string, p: string): string {
    if (typeof p !== 'string') {
        throw new Error('storage path must be a string');
    }
    if (p.includes('\\')) {
        throw new Error('storage path must use forward slashes');
    }
    if (path.isAbsolute(p)) {
        throw new Error('storage path must be relative');
    }
    const joined = path.resolve(root, p);
    const rel = path.relative(root, joined);
    // rel is empty when joined === root (path === '' or '.'); not an escape.
    if (rel.startsWith('..')) {
        throw new Error('storage path escapes root');
    }
    return joined;
}

/** Treat empty / '.' / '/' as the storage root itself. The remove() handler
 *  refuses to operate on the root; list() lists it. */
function isRoot(p: string): boolean {
    return p === '' || p === '.' || p === '/';
}

export function createStorageHost(getRoot: () => string): StorageHost {
    return {
        async read(p) {
            const full = safeJoin(getRoot(), p);
            try {
                const buf = await fs.readFile(full);
                // Node Buffer is a Uint8Array, but its `buffer` may be a slab
                // shared with other Buffers. Copy out for a clean Uint8Array.
                return new Uint8Array(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
            } catch (e) {
                const err = e as NodeJS.ErrnoException;
                if (err.code === 'ENOENT' || err.code === 'EISDIR') return null;
                throw e;
            }
        },

        async write(p, data) {
            const full = safeJoin(getRoot(), p);
            await fs.mkdir(path.dirname(full), { recursive: true });
            await fs.writeFile(full, data);
        },

        async list(dir) {
            const root = getRoot();
            const full = isRoot(dir) ? root : safeJoin(root, dir);
            try {
                const ents = await fs.readdir(full, { withFileTypes: true });
                return ents.map(e => ({
                    name: e.name,
                    kind: e.isDirectory() ? 'directory' as const : 'file' as const,
                }));
            } catch (e) {
                const err = e as NodeJS.ErrnoException;
                if (err.code === 'ENOENT' || err.code === 'ENOTDIR') return [];
                throw e;
            }
        },

        async remove(p) {
            if (isRoot(p)) return;          // refuse to delete the storage root
            const full = safeJoin(getRoot(), p);
            if (full === getRoot()) return; // belt and suspenders
            try {
                await fs.rm(full, { recursive: true, force: true });
            } catch (e) {
                const err = e as NodeJS.ErrnoException;
                if (err.code === 'ENOENT') return;
                throw e;
            }
        },

        async exists(p) {
            const root = getRoot();
            const full = isRoot(p) ? root : safeJoin(root, p);
            try {
                await fs.access(full);
                return true;
            } catch {
                return false;
            }
        },
    };
}
