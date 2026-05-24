/**
 * OPFS-backed DarklyStorage. Used in the hosted web build.
 */
import type { DarklyStorage, DirEntry } from './types';

function opfsAvailable(): boolean {
    return typeof navigator !== 'undefined'
        && 'storage' in navigator
        && typeof navigator.storage.getDirectory === 'function';
}

/** Split a path into segments, dropping empty pieces. */
function segments(path: string): string[] {
    return path.split('/').filter(s => s.length > 0);
}

export class OpfsStorage implements DarklyStorage {
    #root: FileSystemDirectoryHandle | null = null;

    async #getRoot(): Promise<FileSystemDirectoryHandle> {
        if (this.#root) return this.#root;
        if (!opfsAvailable()) {
            throw new Error('OPFS not available — modern browser required');
        }
        this.#root = await navigator.storage.getDirectory();
        return this.#root;
    }

    /** Walk to the directory holding the final segment.
     *  If `create`, intermediate dirs are created; otherwise returns null on
     *  the first missing segment. */
    async #parentDir(
        path: string,
        create: boolean,
    ): Promise<{ parent: FileSystemDirectoryHandle; leaf: string } | null> {
        const parts = segments(path);
        if (parts.length === 0) return null;
        let dir = await this.#getRoot();
        for (let i = 0; i < parts.length - 1; i++) {
            try {
                dir = await dir.getDirectoryHandle(parts[i], { create });
            } catch (e) {
                if ((e as DOMException)?.name === 'NotFoundError') return null;
                throw e;
            }
        }
        return { parent: dir, leaf: parts[parts.length - 1] };
    }

    /** Resolve a path to a directory handle. Returns null if missing. */
    async #resolveDir(path: string): Promise<FileSystemDirectoryHandle | null> {
        const parts = segments(path);
        let dir = await this.#getRoot();
        for (const part of parts) {
            try {
                dir = await dir.getDirectoryHandle(part);
            } catch (e) {
                if ((e as DOMException)?.name === 'NotFoundError') return null;
                throw e;
            }
        }
        return dir;
    }

    async read(path: string): Promise<Uint8Array | null> {
        const loc = await this.#parentDir(path, false);
        if (!loc) return null;
        try {
            const fh = await loc.parent.getFileHandle(loc.leaf);
            const file = await fh.getFile();
            return new Uint8Array(await file.arrayBuffer());
        } catch (e) {
            if ((e as DOMException)?.name === 'NotFoundError') return null;
            throw e;
        }
    }

    async write(path: string, data: Uint8Array): Promise<void> {
        const loc = await this.#parentDir(path, true);
        if (!loc) throw new Error(`Cannot write to root or empty path: ${path}`);
        const fh = await loc.parent.getFileHandle(loc.leaf, { create: true });
        const writer = await fh.createWritable();
        try {
            // See fileHandle.ts::writeToHandle for the cast rationale.
            await writer.write(data as Uint8Array<ArrayBuffer>);
        } finally {
            await writer.close();
        }
    }

    async list(dir: string): Promise<DirEntry[]> {
        const handle = await this.#resolveDir(dir);
        if (!handle) return [];
        const out: DirEntry[] = [];
        for await (const [name, h] of handle as unknown as AsyncIterable<[string, FileSystemHandle]>) {
            out.push({ name, kind: h.kind });
        }
        return out;
    }

    async remove(path: string): Promise<void> {
        const loc = await this.#parentDir(path, false);
        if (!loc) return;
        try {
            await loc.parent.removeEntry(loc.leaf, { recursive: true });
        } catch (e) {
            if ((e as DOMException)?.name === 'NotFoundError') return;
            throw e;
        }
    }

    async exists(path: string): Promise<boolean> {
        const loc = await this.#parentDir(path, false);
        if (!loc) return false;
        try {
            await loc.parent.getFileHandle(loc.leaf);
            return true;
        } catch (e) {
            if ((e as DOMException)?.name !== 'NotFoundError'
                && (e as DOMException)?.name !== 'TypeMismatchError') {
                throw e;
            }
        }
        try {
            await loc.parent.getDirectoryHandle(loc.leaf);
            return true;
        } catch (e) {
            if ((e as DOMException)?.name === 'NotFoundError'
                || (e as DOMException)?.name === 'TypeMismatchError') {
                return false;
            }
            throw e;
        }
    }
}
