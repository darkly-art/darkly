/**
 * Personal font library: the frontend source of truth for every non-fallback
 * font (uploaded, Google-imported, or pulled out of an opened `.darkly`).
 *
 * Fonts live in an IndexedDB store keyed by content hash so they survive reload
 * and are shared across every document you open, complementary to `.darkly`
 * embedding (which makes a single file portable). The library **replays itself
 * into every engine handle**: `registerIntoHandle` is called by the editor
 * bootstrap for each new tab, and `add` pushes a freshly-acquired font into all
 * currently-open tabs. The engine's `FontRegistry` stays per-handle; this module
 * keeps them in sync.
 *
 * Bytes are content-addressed with the same FNV-1a hash the Rust byte cache
 * uses, so a font registers exactly once regardless of how many times its bytes
 * arrive.
 */
import { unzipSync } from 'fflate';
import { shell } from '../multi_tab/shell.svelte';
import type { Engine } from '../engine/protocol';

/** Where a library font came from. `embedded` = extracted from an opened
 *  `.darkly`; it joins the library so it's reusable across documents. */
export type FontSource = 'upload' | 'google' | 'embedded';

interface FontRecord {
    hash: string;
    bytes: Uint8Array;
    families: string[];
    source: FontSource;
}

const DB_NAME = 'darkly-fonts';
const DB_VERSION = 1;
const STORE = 'fonts';

/** Stable 64-bit FNV-1a content hash, hex-encoded: the same algorithm the Rust
 *  `text::content_hash` uses, so a font addresses identically on both sides. */
export function contentHash(bytes: Uint8Array): string {
    let h = 0xcbf29ce484222325n;
    const prime = 0x100000001b3n;
    const mask = 0xffffffffffffffffn;
    for (let i = 0; i < bytes.length; i++) {
        h = (h ^ BigInt(bytes[i])) & mask;
        h = (h * prime) & mask;
    }
    return h.toString(16).padStart(16, '0');
}

function openDb(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
        const req = indexedDB.open(DB_NAME, DB_VERSION);
        req.onupgradeneeded = () => {
            const db = req.result;
            if (!db.objectStoreNames.contains(STORE)) {
                db.createObjectStore(STORE, { keyPath: 'hash' });
            }
        };
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
    });
}

function idbRequest<T>(req: IDBRequest<T>): Promise<T> {
    return new Promise((resolve, reject) => {
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
    });
}

/** Every live engine handle across all open tabs: the replay targets. */
function liveEngines(): Engine[] {
    return shell.instances.map((i) => i.engine).filter((e): e is Engine => e !== null);
}

class FontLibrary {
    /** Every family the library provides, deduped and sorted: the reactive
     *  source the font pickers bind to. Excludes the binary-resident fallback,
     *  which the engine surfaces through `list_fonts` separately. */
    families = $state<string[]>([]);

    /** hash → record, the in-memory mirror of the IndexedDB store. */
    private records = new Map<string, FontRecord>();
    private loaded = false;

    /** Load the persisted library into memory on startup. Idempotent: a second
     *  call is a no-op once the first has populated the mirror. Does not touch
     *  engine handles; each handle pulls the library in via `registerIntoHandle`
     *  at its own creation. */
    async loadAll(): Promise<void> {
        if (this.loaded) return;
        this.loaded = true;
        let all: FontRecord[] = [];
        try {
            const db = await openDb();
            const tx = db.transaction(STORE, 'readonly');
            all = await idbRequest(tx.objectStore(STORE).getAll() as IDBRequest<FontRecord[]>);
            db.close();
        } catch (e) {
            console.warn('[fonts] failed to load library from IndexedDB', e);
            return;
        }
        for (const rec of all) {
            this.records.set(rec.hash, rec);
            this.injectBrowserFont(rec);
        }
        this.refreshFamilies();
    }

    /** Register a font blob into the library: content-hash it, register into
     *  every live handle, and persist. Returns the family names the blob
     *  contributed (empty if no engine accepted it). A font whose bytes are
     *  already known is a no-op beyond returning its families: the content hash
     *  dedups, so re-adding costs nothing. */
    async add(bytes: Uint8Array, source: FontSource): Promise<string[]> {
        const hash = contentHash(bytes);
        const existing = this.records.get(hash);
        if (existing) return existing.families;

        // Register into every open tab; the first non-empty response names the
        // families (identical across handles, same bytes, same collection).
        let families: string[] = [];
        for (const engine of liveEngines()) {
            const res = await engine.api.registerFont(bytes).catch(() => null);
            if (res?.families?.length && families.length === 0) families = res.families;
        }
        if (families.length === 0) {
            console.warn('[fonts] no engine accepted the font blob; not persisting');
            return [];
        }

        const rec: FontRecord = { hash, bytes, families, source };
        this.records.set(hash, rec);
        this.injectBrowserFont(rec);
        this.refreshFamilies();
        try {
            const db = await openDb();
            const tx = db.transaction(STORE, 'readwrite');
            await idbRequest(tx.objectStore(STORE).put(rec));
            db.close();
        } catch (e) {
            console.warn('[fonts] failed to persist font to IndexedDB', e);
        }
        return families;
    }

    /** Replay the whole library into one freshly-created handle. Called by the
     *  editor bootstrap for every new tab so its engine's font collection
     *  matches the library before the first frame. */
    async registerIntoHandle(engine: Engine): Promise<void> {
        await this.loadAll();
        for (const rec of this.records.values()) {
            await engine.api.registerFont(rec.bytes).catch((e) => {
                console.warn('[fonts] replay into handle failed', e);
            });
        }
    }

    /** Absorb the fonts embedded in an opened `.darkly` into the library so
     *  they persist and become reusable across documents. The engine already
     *  registered them into the opening handle during `open_document`; this
     *  makes them survive reload and reach future tabs. Best-effort: a
     *  malformed archive or missing blob is skipped, never surfaced as an error.
     */
    async absorbDarkly(zipBytes: Uint8Array): Promise<void> {
        let entries: Record<string, Uint8Array>;
        try {
            entries = unzipSync(zipBytes);
        } catch {
            return;
        }
        const manifestBytes = entries['manifest.json'];
        if (!manifestBytes) return;
        let fonts: Array<{ hash: string; path: string }> = [];
        try {
            const manifest = JSON.parse(new TextDecoder().decode(manifestBytes));
            fonts = Array.isArray(manifest.fonts) ? manifest.fonts : [];
        } catch {
            return;
        }
        const seen = new Set<string>();
        for (const font of fonts) {
            if (!font?.path || seen.has(font.hash)) continue;
            seen.add(font.hash);
            if (this.records.has(font.hash)) continue;
            const bytes = entries[font.path];
            if (bytes) await this.add(bytes, 'embedded');
        }
    }

    /** True once the bytes behind `hash` are in the library. */
    has(hash: string): boolean {
        return this.records.has(hash);
    }

    /** Register a record's families as browser `FontFace`s so the UI can preview
     *  them in their own typeface (the engine's collection is separate from the
     *  document's CSS). Browser-only + best-effort: a no-op under vitest/node. */
    private injectBrowserFont(rec: FontRecord): void {
        if (typeof document === 'undefined' || !('fonts' in document)) return;
        for (const family of rec.families) {
            try {
                const face = new FontFace(family, rec.bytes as unknown as ArrayBuffer);
                face.load()
                    .then((f) => document.fonts.add(f))
                    .catch(() => {});
            } catch {
                /* FontFace unavailable: previews degrade to the fallback face. */
            }
        }
    }

    private refreshFamilies(): void {
        const set = new Set<string>();
        for (const rec of this.records.values()) {
            for (const f of rec.families) set.add(f);
        }
        this.families = [...set].sort((a, b) => a.localeCompare(b));
    }
}

export const fontLibrary = new FontLibrary();
