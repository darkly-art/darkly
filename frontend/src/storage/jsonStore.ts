/**
 * JSON records in the Darkly directory.
 *
 * Two shapes, one write discipline:
 *   - `jsonFile` — a single document (`recents.json`).
 *   - `jsonDir`  — a directory of id-keyed records (`packs/`, `brushes/`).
 *
 * Writes are coalesced on a trailing edge, so a burst of mutations costs one
 * write, and serialized against each other per path, so two writes can never
 * interleave and leave a torn file. Both properties already existed in this
 * codebase — the coalescing in `config/store.svelte.ts`'s `#scheduleWrite` and
 * the serialization in `recording/recorder.svelte.ts`'s `withScratchLock` —
 * and are factored here rather than copied a third and fourth time.
 *
 * Everything under this directory rides `exportRootAsZip`, so a record written
 * here travels with the painter's settings when they export.
 */
import { readJson, writeJson, storage as defaultStorage } from './index';
import type { DarklyStorage } from './types';

/** Trailing-edge window. Matches `config/store.svelte.ts`'s user-settings
 *  write, for the same reason: long enough to absorb a drag, short enough that
 *  a reload immediately after a change keeps it. */
const WRITE_DEBOUNCE_MS = 200;

/** One in-flight write chain per path. Keyed globally so two handles on the
 *  same file share the chain rather than racing. */
const writeLocks = new Map<string, Promise<unknown>>();

/** Run `fn` exclusively against `path`. FIFO — writes land in issue order. */
function withWriteLock<T>(path: string, fn: () => Promise<T>): Promise<T> {
    const prev = writeLocks.get(path) ?? Promise.resolve();
    const next = prev.then(fn, fn);
    writeLocks.set(path, next.then(() => undefined, () => undefined));
    return next;
}

/** A pending trailing-edge write: the timer, and the value it will write. */
interface Pending<T> {
    timer: ReturnType<typeof setTimeout>;
    value: T;
    settled: Promise<void>;
    resolve: () => void;
}

/** Schedule `value` to be written to `path`, coalescing with any write already
 *  pending for it. Returns a promise that settles when the write lands. */
function commit<T>(path: string, entry: Pending<T>, s: DarklyStorage): void {
    void withWriteLock(path, async () => {
        try {
            await writeJson(path, entry.value, s);
        } catch (e) {
            console.error(`[storage] write failed for ${path}`, e);
        }
    }).finally(() => entry.resolve());
}

function schedule<T>(
    pending: Map<string, Pending<T>>,
    path: string,
    value: T,
    s: DarklyStorage,
): void {
    const existing = pending.get(path);
    if (existing) {
        // Coalesce: the last value in the window wins, and the timer already
        // running keeps its deadline so a steady stream still drains.
        existing.value = value;
        return;
    }

    let resolve!: () => void;
    const settled = new Promise<void>(r => { resolve = r; });

    const timer = setTimeout(() => {
        const entry = pending.get(path);
        pending.delete(path);
        if (entry) commit(path, entry, s);
    }, WRITE_DEBOUNCE_MS);

    pending.set(path, { timer, value, settled, resolve });
}

/** Flush every pending write in `pending` immediately, and wait for them. */
async function flushAll<T>(
    pending: Map<string, Pending<T>>,
    s: DarklyStorage,
): Promise<void> {
    const entries = [...pending.entries()];
    for (const [path, entry] of entries) {
        clearTimeout(entry.timer);
        pending.delete(path);
        commit(path, entry, s);
    }
    await Promise.all(entries.map(([, e]) => e.settled));
}

export interface JsonFile<T> {
    /** Read the file. A missing file, malformed JSON, or a value that fails
     *  `validate` all read as the fallback — never a throw. */
    read(): Promise<T>;
    /** Queue a coalesced write. Fire-and-forget by design. */
    write(value: T): void;
    /** Write anything pending now and wait for it to land. */
    flush(): Promise<void>;
}

/**
 * A single JSON file in the Darkly directory.
 *
 * `fallback` supplies the value for a file that is missing or unreadable, and
 * `validate` (when given) has the last word on whether what was read is usable
 * — a stored file may be arbitrarily old or hand-edited.
 */
export function jsonFile<T>(
    path: string,
    fallback: () => T,
    validate?: (raw: unknown) => T | null,
    s: DarklyStorage = defaultStorage,
): JsonFile<T> {
    const pending = new Map<string, Pending<T>>();

    return {
        async read(): Promise<T> {
            let raw: unknown;
            try {
                raw = await readJson(path, s);
            } catch (e) {
                console.warn(`[storage] read failed for ${path}`, e);
                return fallback();
            }
            if (raw === null || raw === undefined) return fallback();
            if (validate) return validate(raw) ?? fallback();
            return raw as T;
        },
        write(value: T): void {
            schedule(pending, path, value, s);
        },
        flush(): Promise<void> {
            return flushAll(pending, s);
        },
    };
}

export interface JsonDir<T> {
    /** Every record that parses, keyed by id. Records that fail to parse are
     *  skipped with a warning — one corrupt file must not cost the caller the
     *  whole directory. */
    readAll(): Promise<Map<string, T>>;
    /** Queue a coalesced write of one record. */
    write(id: string, value: T): void;
    /** Delete one record. Idempotent. */
    remove(id: string): Promise<void>;
    /** Write anything pending now and wait for it to land. */
    flush(): Promise<void>;
}

/**
 * A directory of id-keyed JSON records, one file per record.
 *
 * There is deliberately no index file. The filename is the id and the id never
 * changes, so a rename rewrites one file in place, a delete removes one file,
 * and nothing can be orphaned or left disagreeing with an index. This is the
 * same reasoning `storage/recovery.ts` states for crash snapshots.
 */
export function jsonDir<T>(
    dir: string,
    validate?: (raw: unknown) => T | null,
    s: DarklyStorage = defaultStorage,
): JsonDir<T> {
    const pending = new Map<string, Pending<T>>();
    const pathOf = (id: string) => `${dir}/${id}.json`;

    return {
        async readAll(): Promise<Map<string, T>> {
            const out = new Map<string, T>();
            let entries;
            try {
                entries = await s.list(dir);
            } catch (e) {
                console.warn(`[storage] list failed for ${dir}`, e);
                return out;
            }
            for (const entry of entries) {
                if (entry.kind !== 'file' || !entry.name.endsWith('.json')) continue;
                const id = entry.name.slice(0, -'.json'.length);
                let raw: unknown;
                try {
                    raw = await readJson(`${dir}/${entry.name}`, s);
                } catch (e) {
                    console.warn(`[storage] skipping unreadable record ${dir}/${entry.name}`, e);
                    continue;
                }
                if (raw === null || raw === undefined) {
                    console.warn(`[storage] skipping malformed record ${dir}/${entry.name}`);
                    continue;
                }
                const value = validate ? validate(raw) : (raw as T);
                if (value === null) {
                    console.warn(`[storage] skipping invalid record ${dir}/${entry.name}`);
                    continue;
                }
                out.set(id, value);
            }
            return out;
        },
        write(id: string, value: T): void {
            schedule(pending, pathOf(id), value, s);
        },
        async remove(id: string): Promise<void> {
            const path = pathOf(id);
            // Drop any queued write first, or it would recreate the file.
            const entry = pending.get(path);
            if (entry) {
                clearTimeout(entry.timer);
                pending.delete(path);
                entry.resolve();
            }
            await withWriteLock(path, () => s.remove(path));
        },
        flush(): Promise<void> {
            return flushAll(pending, s);
        },
    };
}
