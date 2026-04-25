/**
 * The Darkly directory: a single FileSystemDirectoryHandle that holds
 * everything the app persists. OPFS-rooted by default; a future upgrade
 * path swaps in a user-granted handle from `showDirectoryPicker()`.
 *
 * Modules that need persistence each own their own subtree under this root
 * and don't know about each other:
 *
 *   /presets/        config presets (the only consumer for now)
 *   /brushes/        future
 *   /recordings/     future
 *
 * Acquire the root via `getRoot()`. For a subdirectory, use `getDir(name)`
 * — it creates the subdir if missing.
 */

let root: FileSystemDirectoryHandle | null = null;

function opfsAvailable(): boolean {
    return typeof navigator !== 'undefined'
        && 'storage' in navigator
        && typeof navigator.storage.getDirectory === 'function';
}

/** Get the root handle. Resolves to OPFS today; future versions may resolve
 *  to a user-granted directory if one was previously chosen. */
export async function getRoot(): Promise<FileSystemDirectoryHandle> {
    if (root) return root;
    if (!opfsAvailable()) {
        throw new Error('OPFS not available — modern browser required');
    }
    root = await navigator.storage.getDirectory();
    return root;
}

/** Get or create a subdirectory of the root. */
export async function getDir(name: string): Promise<FileSystemDirectoryHandle> {
    const r = await getRoot();
    return r.getDirectoryHandle(name, { create: true });
}

/** Read a text file. Returns null if not found. */
export async function readText(
    dir: FileSystemDirectoryHandle,
    name: string,
): Promise<string | null> {
    try {
        const fh = await dir.getFileHandle(name);
        const file = await fh.getFile();
        return await file.text();
    } catch (e) {
        if ((e as DOMException)?.name === 'NotFoundError') return null;
        throw e;
    }
}

/** Read JSON. Returns null if file not found or content fails to parse. */
export async function readJson<T = unknown>(
    dir: FileSystemDirectoryHandle,
    name: string,
): Promise<T | null> {
    const text = await readText(dir, name);
    if (text === null) return null;
    try { return JSON.parse(text) as T; }
    catch { return null; }
}

/** Write a text file (creating it if necessary). */
export async function writeText(
    dir: FileSystemDirectoryHandle,
    name: string,
    contents: string,
): Promise<void> {
    const fh = await dir.getFileHandle(name, { create: true });
    const writer = await fh.createWritable();
    try {
        await writer.write(contents);
    } finally {
        await writer.close();
    }
}

/** Write a JSON file. */
export async function writeJson(
    dir: FileSystemDirectoryHandle,
    name: string,
    value: unknown,
): Promise<void> {
    await writeText(dir, name, JSON.stringify(value, null, 2));
}

/** List entries in a directory. */
export async function listEntries(
    dir: FileSystemDirectoryHandle,
): Promise<{ name: string; kind: 'file' | 'directory' }[]> {
    const out: { name: string; kind: 'file' | 'directory' }[] = [];
    // Async iterator on FileSystemDirectoryHandle.
    // TypeScript's lib.dom doesn't always include this signature, so we
    // cast through unknown.
    for await (const [name, handle] of dir as unknown as AsyncIterable<[string, FileSystemHandle]>) {
        out.push({ name, kind: handle.kind });
    }
    return out;
}

/** Remove a file or directory by name. Idempotent. */
export async function removeEntry(
    dir: FileSystemDirectoryHandle,
    name: string,
): Promise<void> {
    try {
        await dir.removeEntry(name, { recursive: true });
    } catch (e) {
        if ((e as DOMException)?.name !== 'NotFoundError') throw e;
    }
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
