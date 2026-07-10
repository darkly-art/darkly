import { describe, it, expect, vi, afterEach } from 'vitest';
import { HttpStreamSource, describeStreamError } from '../httpStreamSource';
import type { Engine } from '../../engine/protocol';

// `HttpStreamSource` reads a length-prefixed WebP stream over `fetch` and decodes
// each frame off-thread via the global `createImageBitmap`. Node vitest has
// neither `fetch` nor `createImageBitmap`, so we stub both: `createImageBitmap`
// returns a fake bitmap, and a controllable reader lets the test deliver frames
// (or close the stream) on demand, interleaved with `tick()` calls.

afterEach(() => vi.unstubAllGlobals());

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** `[4-byte big-endian length][payload]` — the on-wire frame format. */
function lenPrefixed(payload: Uint8Array): Uint8Array {
    const out = new Uint8Array(4 + payload.length);
    new DataView(out.buffer).setUint32(0, payload.length, false);
    out.set(payload, 4);
    return out;
}

/** A reader whose `read()` resolves only when the test pushes a chunk or closes,
 *  so frame delivery can be interleaved with ticks. */
function controllableReader() {
    const waiters: Array<(r: { done: boolean; value?: Uint8Array }) => void> = [];
    const buffered: Array<{ done: boolean; value?: Uint8Array }> = [];
    const deliver = (r: { done: boolean; value?: Uint8Array }) => {
        const w = waiters.shift();
        if (w) w(r);
        else buffered.push(r);
    };
    return {
        reader: {
            read: () =>
                new Promise<{ done: boolean; value?: Uint8Array }>((resolve) => {
                    const b = buffered.shift();
                    if (b) resolve(b);
                    else waiters.push(resolve);
                }),
        },
        push: (value: Uint8Array) => deliver({ done: false, value }),
        close: () => deliver({ done: true }),
    };
}

function harness() {
    const uploads: number[] = [];
    const decodeOptions: Array<ImageBitmapOptions | undefined> = [];
    let bitmapId = 0;
    vi.stubGlobal('createImageBitmap', (_source: Blob, options?: ImageBitmapOptions) => {
        decodeOptions.push(options);
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
    return { engine, uploads, decodeOptions, push, close, fetchCalls };
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

describe('describeStreamError', () => {
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
