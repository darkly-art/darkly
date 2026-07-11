import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { HttpStreamSource, StreamStalledError, describeStreamError } from '../httpStreamSource';
import type { Engine } from '../../engine/protocol';
import { lenPrefixed, controllableReader, HEARTBEAT } from './streamTestUtils';

// `HttpStreamSource` reads a length-prefixed WebP stream over `fetch` and decodes
// each frame off-thread via the global `createImageBitmap`. Node vitest has
// neither `fetch` nor `createImageBitmap`, so we stub both: `createImageBitmap`
// returns a fake bitmap, and a controllable reader lets the test deliver frames
// (or close the stream) on demand, interleaved with `tick()` calls.

afterEach(() => vi.unstubAllGlobals());

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function harness() {
    const uploads: number[] = [];
    const decodeOptions: Array<ImageBitmapOptions | undefined> = [];
    const decodeSources: Blob[] = [];
    let bitmapId = 0;
    vi.stubGlobal('createImageBitmap', (source: Blob, options?: ImageBitmapOptions) => {
        decodeOptions.push(options);
        decodeSources.push(source);
        return Promise.resolve({ width: 8, height: 8, close: () => {}, id: ++bitmapId });
    });
    const engine = {
        uploadVoidExternalImage: (layerId: number) => uploads.push(layerId),
    } as unknown as Engine;
    const { reader, push, close } = controllableReader();
    const fetchCalls: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
        fetchCalls.push(url);
        return Promise.resolve({ ok: true, body: { getReader: () => reader } });
    });
    return { engine, uploads, decodeOptions, decodeSources, push, close, fetchCalls };
}

const WEBP = new Uint8Array([1, 2, 3, 4, 5]); // stand-in frame payload

describe('HttpStreamSource frame parsing + upload', () => {
    it('uploads one frame per delivered frame, honoring the divisor gate', async () => {
        const { engine, uploads, push } = harness();
        const src = new HttpStreamSource(9, engine);
        await src.start('http://localhost:8765/stream');
        await flush();

        // Frame 1 arrives; a tick on a divisor-aligned frame count uploads it.
        push(lenPrefixed(WEBP));
        await flush();
        src.tick(4); // 4 % 4 === 0
        await flush();
        expect(uploads).toEqual([9]);

        // No new frame yet → the dedup gate suppresses further uploads even on
        // an aligned tick (a static scene drives zero decode + GPU work).
        src.tick(8);
        await flush();
        expect(uploads).toEqual([9]);

        // Frame 2 arrives; the next aligned tick uploads it.
        push(lenPrefixed(WEBP));
        await flush();
        src.tick(8);
        await flush();
        expect(uploads).toEqual([9, 9]);

        src.stop();
    });

    it('reassembles a frame split across chunk boundaries', async () => {
        const { engine, uploads, push } = harness();
        const src = new HttpStreamSource(1, engine);
        await src.start('http://localhost:8765/stream');
        await flush();

        const framed = lenPrefixed(WEBP);
        push(framed.slice(0, 3)); // partial: length prefix not even complete
        await flush();
        src.tick(4);
        await flush();
        expect(uploads).toEqual([]); // nothing decodable yet

        push(framed.slice(3)); // the rest
        await flush();
        src.tick(4);
        await flush();
        expect(uploads).toEqual([1]);

        src.stop();
    });

    it('suppresses uploads when frozen or not visible', async () => {
        const { engine, uploads, push } = harness();
        const src = new HttpStreamSource(2, engine);
        await src.start('http://localhost:8765/stream');
        await flush();
        push(lenPrefixed(WEBP));
        await flush();

        src.setFrozen(true);
        src.tick(4);
        await flush();
        expect(uploads).toEqual([]);

        src.setFrozen(false);
        src.setVisible(false);
        src.tick(4);
        await flush();
        expect(uploads).toEqual([]);

        // Visible + unfrozen + aligned → the buffered frame finally uploads.
        src.setVisible(true);
        src.tick(4);
        await flush();
        expect(uploads).toEqual([2]);

        src.stop();
    });

    it('skips ticks that miss the divisor gate', async () => {
        const { engine, uploads, push } = harness();
        const src = new HttpStreamSource(3, engine);
        await src.start('http://localhost:8765/stream');
        await flush();
        push(lenPrefixed(WEBP));
        await flush();

        src.setFrameDivisor(4);
        src.tick(5); // 5 % 4 !== 0
        await flush();
        expect(uploads).toEqual([]);

        src.stop();
    });

    it('decodes frames into the premultiplied convention the frame texture stores', async () => {
        // Convention pin: the void's aux texture holds premultiplied texels
        // (so GPU linear filtering doesn't darken alpha edges — see
        // `video_stream_void.rs`); the straight-alpha frames the add-on emits
        // must be converted at decode. Guards a drive-by revert to 'none';
        // the behavioral regression test lives in `tests/void_layer.rs`.
        const { engine, uploads, decodeOptions, push } = harness();
        const src = new HttpStreamSource(6, engine);
        await src.start('http://localhost:8765/stream');
        await flush();

        push(lenPrefixed(WEBP));
        await flush();
        src.tick(4);
        await flush();
        expect(uploads).toEqual([6]);
        expect(decodeOptions).toHaveLength(1);
        expect(decodeOptions[0]?.premultiplyAlpha).toBe('premultiply');

        src.stop();
    });
});

describe('HttpStreamSource disconnect', () => {
    it('fires onEnded and flips `ended` when the stream closes', async () => {
        const { engine, close } = harness();
        const ended: number[] = [];
        const src = new HttpStreamSource(7, engine, (id) => ended.push(id));
        await src.start('http://localhost:8765/stream');
        await flush();

        expect(src.ended).toBe(false);
        close();
        await flush();
        expect(src.ended).toBe(true);
        expect(ended).toEqual([7]);
        expect(src.error).toContain('disconnected');
    });

    it('reports a connect failure when fetch rejects', async () => {
        const ended: number[] = [];
        vi.stubGlobal('createImageBitmap', () => Promise.resolve({ close: () => {} }));
        vi.stubGlobal('fetch', () => Promise.reject(new TypeError('Failed to fetch')));
        const engine = { uploadVoidExternalImage: () => {} } as unknown as Engine;
        const src = new HttpStreamSource(11, engine, (id) => ended.push(id));
        await src.start('http://localhost:9999/stream');
        await flush();

        expect(src.ended).toBe(true);
        expect(ended).toEqual([11]);
        expect(src.error).toContain('Is the add-on running?');
    });

    it('does not fire onEnded after an explicit stop', async () => {
        const { engine, close } = harness();
        const ended: number[] = [];
        const src = new HttpStreamSource(4, engine, (id) => ended.push(id));
        await src.start('http://localhost:8765/stream');
        await flush();

        src.stop();
        close(); // server close racing the local teardown
        await flush();
        expect(ended).toEqual([]);
    });
});

// The wire protocol only sends bytes on a scene change, so liveness rests on
// heartbeats (zero-length frames) plus a client-side stall watchdog. Fake
// timers drive both the watchdog interval and `Date.now()`.
describe('HttpStreamSource heartbeats + stall watchdog', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('declares a byte-silent stream dead and reports it', async () => {
        // Regression: a dead-but-open socket used to await `reader.read()`
        // forever — indistinguishable from an idle scene, no error surfaced.
        const { engine } = harness();
        const ended: number[] = [];
        const src = new HttpStreamSource(5, engine, (id) => ended.push(id));
        await src.start('http://localhost:8765/stream');

        // Total byte silence past the stall timeout → disconnect.
        await vi.advanceTimersByTimeAsync(10_000);
        expect(ended).toEqual([5]);
        expect(src.ended).toBe(true);
        expect(src.error).toContain('stopped responding');
        expect(src.status).toBe('disconnected');
    });

    it('heartbeats keep the stream alive and never trigger uploads', async () => {
        const { engine, uploads, push } = harness();
        const ended: number[] = [];
        const src = new HttpStreamSource(6, engine, (id) => ended.push(id));
        await src.start('http://localhost:8765/stream');

        // Heartbeats-only for well past the stall timeout: still alive.
        for (let i = 0; i < 12; i++) {
            push(HEARTBEAT);
            await vi.advanceTimersByTimeAsync(2000);
        }
        expect(ended).toEqual([]);
        expect(src.ended).toBe(false);

        // A heartbeat is not a frame — nothing to decode or upload.
        src.tick(4);
        await vi.advanceTimersByTimeAsync(0);
        expect(uploads).toEqual([]);

        src.stop();
    });

    it('a heartbeat does not clobber a pending real frame', async () => {
        const { engine, uploads, decodeSources, push } = harness();
        const src = new HttpStreamSource(7, engine);
        await src.start('http://localhost:8765/stream');

        push(lenPrefixed(WEBP));
        push(HEARTBEAT);
        await vi.advanceTimersByTimeAsync(0);
        src.tick(4);
        await vi.advanceTimersByTimeAsync(0);
        // Exactly one upload, and it decoded the real frame's bytes — not the
        // heartbeat's empty payload.
        expect(uploads).toEqual([7]);
        expect(decodeSources).toHaveLength(1);
        expect(decodeSources[0].size).toBe(WEBP.length);

        // The heartbeat did not re-arm the new-frame flag either.
        src.tick(8);
        await vi.advanceTimersByTimeAsync(0);
        expect(uploads).toEqual([7]);

        src.stop();
    });

    it('walks connecting → connected → disconnected, notifying each transition', async () => {
        const { engine, close } = harness();
        const statuses: string[] = [];
        const src = new HttpStreamSource(8, engine, null, () => statuses.push(src.status));
        // Created-for-immediate-start: connecting from the outset.
        expect(src.status).toBe('connecting');

        await src.start('http://localhost:8765/stream');
        expect(src.status).toBe('connected');

        close();
        await vi.advanceTimersByTimeAsync(0);
        expect(src.status).toBe('disconnected');
        expect(statuses).toEqual(['connected', 'disconnected']);
    });

    it('setUrl mid-stream supersedes the old pump and rearms the watchdog', async () => {
        // Per-connection readers: each fetch gets its own, so the old and new
        // connections can be driven independently.
        const uploads: number[] = [];
        vi.stubGlobal('createImageBitmap', () =>
            Promise.resolve({ width: 8, height: 8, close: () => {} }),
        );
        const engine = {
            uploadVoidExternalImage: (layerId: number) => uploads.push(layerId),
        } as unknown as Engine;
        const connections: Array<ReturnType<typeof controllableReader>> = [];
        vi.stubGlobal('fetch', () => {
            const conn = controllableReader();
            connections.push(conn);
            return Promise.resolve({ ok: true, body: { getReader: () => conn.reader } });
        });

        const ended: number[] = [];
        const src = new HttpStreamSource(12, engine, (id) => ended.push(id));
        await src.start('http://localhost:8765/stream');
        expect(connections).toHaveLength(1);

        src.setUrl('http://localhost:9999/stream');
        await vi.advanceTimersByTimeAsync(0);
        expect(connections).toHaveLength(2);

        // Exactly one live stall interval after the supersede — the old
        // connect's watchdog was cleared, not orphaned.
        expect(vi.getTimerCount()).toBe(1);

        // The superseded pump bows out silently: closing it is not an end.
        connections[0].close();
        await vi.advanceTimersByTimeAsync(0);
        expect(ended).toEqual([]);
        expect(src.ended).toBe(false);

        // The new connection is the live one.
        connections[1].push(lenPrefixed(WEBP));
        await vi.advanceTimersByTimeAsync(0);
        src.tick(4);
        await vi.advanceTimersByTimeAsync(0);
        expect(uploads).toEqual([12]);

        src.stop();
        expect(vi.getTimerCount()).toBe(0);
    });
});

describe('describeStreamError', () => {
    it('words a stall', () => {
        const msg = describeStreamError(
            new StreamStalledError('stall'),
            'http://localhost:8765/stream',
        );
        expect(msg).toContain('stopped responding');
        expect(msg).toContain('Is Blender still running?');
    });

    it('words a clean disconnect', () => {
        expect(describeStreamError(null, 'http://localhost:8765/stream')).toBe(
            'Blender stream at http://localhost:8765/stream disconnected.',
        );
    });

    it('flags an unreachable server (TypeError from fetch)', () => {
        const msg = describeStreamError(new TypeError('Failed to fetch'), 'http://localhost:1/x');
        expect(msg).toContain('Could not connect');
        expect(msg).toContain('Is the add-on running?');
    });

    it('passes through other error messages', () => {
        expect(describeStreamError(new Error('boom'), 'http://x')).toBe('Blender stream error: boom');
    });
});
