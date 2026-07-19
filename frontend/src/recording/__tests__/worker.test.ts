/**
 * Regression test for rolled/flushed segments losing their `.json` metadata.
 *
 * The bug lived in the *worker's own* dispatch/teardown ordering: it launched
 * the `segment-<n>.json` write fire-and-forget, then posted `'closed'` /
 * `'flushed'` and (on close) tore the worker down before the write's first
 * `await` resumed — guillotining it. So the test must drive the *real*
 * `worker.ts`; a fake worker that already awaits the write would pass before
 * and after the fix and prove nothing.
 *
 * The real worker needs `self`, `navigator.storage`, and `VideoEncoder`,
 * none of which exist in the Vitest node environment — so we stub them with
 * an in-memory OPFS fake whose sync-access-handle acquisition awaits a
 * microtask (mirroring real OPFS async), making the fire-and-forget ordering
 * observable: at the moment `'closed'`/`'flushed'` is posted we snapshot the
 * fake's `segment-0.json` and assert it is already complete on disk.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { SCRATCH_ROOT, segmentBinName, segmentJsonName } from '../segments';

// --- in-memory OPFS fake --------------------------------------------------

/** One file: a growable byte buffer, or null until first written. */
class FakeFile {
    bytes: Uint8Array | null = null;

    // The sole async hop the worker's writes await; mirrors real OPFS so the
    // fire-and-forget path is observably still in flight when the worker
    // posts its terminal message.
    async createSyncAccessHandle() {
        await Promise.resolve();
        const file = this;
        return {
            truncate(size: number): void {
                const cur = file.bytes ?? new Uint8Array(0);
                file.bytes = cur.slice(0, size);
            },
            write(buf: Uint8Array, opts?: { at?: number }): number {
                const at = opts?.at ?? 0;
                const end = at + buf.length;
                let cur = file.bytes ?? new Uint8Array(0);
                if (end > cur.length) {
                    const grown = new Uint8Array(end);
                    grown.set(cur);
                    cur = grown;
                }
                cur.set(buf, at);
                file.bytes = cur;
                return buf.length;
            },
            getSize(): number {
                return file.bytes?.length ?? 0;
            },
            flush(): void {},
            close(): void {},
        };
    }
}

class FakeDir {
    dirs = new Map<string, FakeDir>();
    files = new Map<string, FakeFile>();

    // No `await` before the child is created, so the handle exists
    // synchronously at call time — only `createSyncAccessHandle` is async.
    async getDirectoryHandle(name: string): Promise<FakeDir> {
        let d = this.dirs.get(name);
        if (!d) {
            d = new FakeDir();
            this.dirs.set(name, d);
        }
        return d;
    }
    async getFileHandle(name: string): Promise<FakeFile> {
        let f = this.files.get(name);
        if (!f) {
            f = new FakeFile();
            this.files.set(name, f);
        }
        return f;
    }
}

// --- fake WebCodecs -------------------------------------------------------

class FakeVideoEncoder {
    state: 'unconfigured' | 'configured' | 'closed' = 'unconfigured';
    constructor(_init: unknown) {}
    configure(_cfg: unknown): void {
        this.state = 'configured';
    }
    encode(_frame: unknown, _opts?: unknown): void {}
    // Emits no chunks — frame plumbing isn't under test, segment-finalize
    // ordering is.
    async flush(): Promise<void> {}
    close(): void {
        this.state = 'closed';
    }
}

// --- harness --------------------------------------------------------------

const SCRATCH_KEY = 'session~recovery';

interface Harness {
    onmessage: (e: { data: unknown }) => void;
    posted: Array<{ type: string }>;
    /** `segment-0.json` bytes captured at the instant `'closed'`/`'flushed'`
     *  was posted — null if the file wasn't written yet. */
    snapshotAt: Record<string, Uint8Array | null>;
    root: FakeDir;
}

function readSegmentJsonSync(root: FakeDir, n: number): Uint8Array | null {
    const key = root.dirs.get(SCRATCH_ROOT)?.dirs.get(SCRATCH_KEY);
    const bytes = key?.files.get(segmentJsonName(n))?.bytes;
    return bytes ? new Uint8Array(bytes) : null;
}

async function loadWorker(): Promise<Harness> {
    const root = new FakeDir();
    const posted: Array<{ type: string }> = [];
    const snapshotAt: Record<string, Uint8Array | null> = {};

    const selfStub = {
        onmessage: null as null | ((e: { data: unknown }) => void),
        postMessage(msg: { type: string }): void {
            posted.push(msg);
            if (msg.type === 'closed' || msg.type === 'flushed') {
                snapshotAt[msg.type] = readSegmentJsonSync(root, 0);
            }
        },
        close(): void {},
    };

    const { vi } = await import('vitest');
    vi.stubGlobal('self', selfStub);
    vi.stubGlobal('navigator', { storage: { getDirectory: async () => root } });
    vi.stubGlobal('VideoEncoder', FakeVideoEncoder);
    vi.stubGlobal('VideoFrame', class {});

    vi.resetModules();
    await import('../worker');

    if (!selfStub.onmessage) throw new Error('worker did not register onmessage');
    return { onmessage: selfStub.onmessage, posted, snapshotAt, root };
}

function initMsg() {
    return {
        type: 'init',
        scratchKey: SCRATCH_KEY,
        segmentN: 0,
        codec: 'avc1.640028',
        width: 16,
        height: 16,
        canvasWidth: 16,
        canvasHeight: 16,
        bitrate: 1_000_000,
        fps: 30,
    };
}

async function waitFor(pred: () => boolean, label: string): Promise<void> {
    for (let i = 0; i < 200; i++) {
        if (pred()) return;
        await new Promise((r) => setTimeout(r, 0));
    }
    throw new Error(`timed out waiting for ${label}`);
}

beforeEach(async () => {
    const { vi } = await import('vitest');
    vi.unstubAllGlobals();
});
afterEach(async () => {
    const { vi } = await import('vitest');
    vi.unstubAllGlobals();
});

describe('encoder worker — segment metadata persistence', () => {
    it('close (roll) persists segment-<n>.json before posting "closed"', async () => {
        const h = await loadWorker();
        h.onmessage({ data: initMsg() });
        await waitFor(() => h.posted.some((m) => m.type === 'ready'), 'ready');

        h.onmessage({ data: { type: 'close' } });
        await waitFor(() => h.posted.some((m) => m.type === 'closed'), 'closed');

        const snap = h.snapshotAt['closed'];
        expect(snap, 'segment-0.json must exist when "closed" is posted').not.toBeNull();
        const meta = JSON.parse(new TextDecoder().decode(snap!));
        expect(meta.n).toBe(0);
        expect(meta.canvasWidth).toBe(16);
    });

    it('flush persists segment-<n>.json before posting "flushed"', async () => {
        const h = await loadWorker();
        h.onmessage({ data: initMsg() });
        await waitFor(() => h.posted.some((m) => m.type === 'ready'), 'ready');

        h.onmessage({ data: { type: 'flush' } });
        await waitFor(() => h.posted.some((m) => m.type === 'flushed'), 'flushed');

        const snap = h.snapshotAt['flushed'];
        expect(snap, 'segment-0.json must exist when "flushed" is posted').not.toBeNull();
        const meta = JSON.parse(new TextDecoder().decode(snap!));
        expect(meta.n).toBe(0);
    });
});
