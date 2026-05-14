/**
 * Storage entry point. Picks the implementation at startup based on
 * whether a host bridge is present (Electron) or not (web), and exposes
 * higher-level helpers (JSON, text, zip export) built on the interface.
 */
import { zip } from 'fflate';
import type { DarklyStorage, DirEntry } from './types';
import { OpfsStorage } from './opfs';
import { NodeFsStorage } from './node';

function pickImpl(): DarklyStorage {
    if (typeof window !== 'undefined' && window.electronAPI?.storage) {
        return new NodeFsStorage();
    }
    return new OpfsStorage();
}

export const storage: DarklyStorage = pickImpl();

export type { DarklyStorage, DirEntry, ElectronStorageBridge } from './types';

// ---------------------------------------------------------------------------
// Text / JSON helpers
// ---------------------------------------------------------------------------

const textDecoder = new TextDecoder();
const textEncoder = new TextEncoder();

/** Read a UTF-8 text file. Returns null if not found. */
export async function readText(path: string): Promise<string | null> {
    const bytes = await storage.read(path);
    return bytes ? textDecoder.decode(bytes) : null;
}

/** Read JSON. Returns null if file not found or content fails to parse. */
export async function readJson<T = unknown>(path: string): Promise<T | null> {
    const text = await readText(path);
    if (text === null) return null;
    try { return JSON.parse(text) as T; }
    catch { return null; }
}

/** Write a UTF-8 text file. */
export async function writeText(path: string, contents: string): Promise<void> {
    await storage.write(path, textEncoder.encode(contents));
}

/** Write a JSON file (pretty-printed). */
export async function writeJson(path: string, value: unknown): Promise<void> {
    await writeText(path, JSON.stringify(value, null, 2));
}

/** Sanitize a user-supplied name into something safe to use as a filename
 *  inside the Darkly directory. Strips path separators, control chars, and
 *  trims to a sane length. */
export function sanitizeFilename(name: string): string {
    return name
        // eslint-disable-next-line no-control-regex
        .replace(/[\x00-\x1f\x7f/\\:*?"<>|]/g, '_')
        .trim()
        .slice(0, 80);
}

// ---------------------------------------------------------------------------
// Zip export
// ---------------------------------------------------------------------------

/** Walk every file under a directory, yielding `[path, bytes]`. Paths use
 *  forward-slash separators and are relative to the walk root. */
async function* walkFiles(dir: string): AsyncIterable<[string, Uint8Array]> {
    const entries = await storage.list(dir);
    for (const entry of entries) {
        const childPath = dir ? `${dir}/${entry.name}` : entry.name;
        if (entry.kind === 'file') {
            const bytes = await storage.read(childPath);
            if (bytes) yield [childPath, bytes];
        } else {
            yield* walkFiles(childPath);
        }
    }
}

/**
 * Bundle the entire Darkly directory into a single Zip blob.
 *
 * Note: builds the whole archive in memory. Fine for our scale (settings +
 * preset JSONs + brush bundles). If we ever need 1 GB+ archives, switch to
 * fflate's streaming Zip constructor.
 */
export async function exportRootAsZip(): Promise<Blob> {
    const entries: Record<string, Uint8Array> = {};
    for await (const [path, bytes] of walkFiles('')) {
        entries[path] = bytes;
    }
    const data: Uint8Array = await new Promise((resolve, reject) => {
        zip(entries, { level: 6 }, (err, out) => {
            if (err) reject(err);
            else resolve(out);
        });
    });
    return new Blob([data], { type: 'application/zip' });
}

/** Download a Blob as a file by triggering a one-shot anchor click. */
export function downloadBlob(blob: Blob, filename: string) {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    // Browsers keep the URL valid until the next tick; revoking immediately
    // would race the download dispatch in Safari.
    setTimeout(() => URL.revokeObjectURL(url), 5000);
}
