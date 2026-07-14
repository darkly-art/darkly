import { describe, it, expect, afterEach, vi } from 'vitest';
import { DarklyInstance } from '../app.svelte';
import { controllableReader } from '../../lib/__tests__/streamTestUtils';

// App-state lifecycle for stream-backed voids: a source that dies externally
// must remain observable (error + status) rather than vanish, and the
// Connect/Resume gesture must be able to replace a dead or failed source.

afterEach(() => vi.unstubAllGlobals());

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function setup() {
    const inst = new DarklyInstance();
    // Enough engine for `startStreamSource`; uploads never happen (no ticks).
    inst.engine = { uploadVoidExternalImage: () => {} } as any;
    // rAF + engine touch — neither exists in the node test env.
    inst.requestFrame = vi.fn();
    inst.refreshLayerTree = vi.fn(async () => {}) as any;
    return inst;
}

/** Stub `fetch` to hand out a fresh controllable reader per connection. */
function stubStreamFetch() {
    const connections: Array<ReturnType<typeof controllableReader>> = [];
    vi.stubGlobal('createImageBitmap', () => Promise.resolve({ close: () => {} }));
    vi.stubGlobal('fetch', () => {
        const conn = controllableReader();
        connections.push(conn);
        return Promise.resolve({ ok: true, body: { getReader: () => conn.reader } });
    });
    return connections;
}

describe('stream source disconnect persistence', () => {
    it('keeps a dead source in the map with error + status; clears the session opt-in', async () => {
        // Regression: `onStreamSourceEnded` used to prune the source, so the
        // `error` string vanished before VoidProperties could render it.
        const inst = setup();
        const connections = stubStreamFetch();

        inst.markStreamVoidStarted(5);
        await inst.startStreamSource(5, 'stream');
        await flush();
        expect(inst.streamSourceFor(5)).not.toBeNull();
        expect(inst.streamSessionStarted.has(5)).toBe(true);

        connections[0].close(); // Blender ended the stream
        await flush();

        const src = inst.streamSourceFor(5);
        expect(src).not.toBeNull();
        expect(src!.ended).toBe(true);
        expect(src!.error).toContain('disconnected');
        expect(src!.status).toBe('disconnected');
        // The opt-in resets so the Connect button reappears.
        expect(inst.streamSessionStarted.has(5)).toBe(false);
        expect(inst.requestFrame).toHaveBeenCalled();
    });

    it('stopStreamSource still prunes (delete / undo / orphan path)', async () => {
        const inst = setup();
        stubStreamFetch();

        await inst.startStreamSource(5, 'stream');
        await flush();
        inst.stopStreamSource(5);
        expect(inst.streamSourceFor(5)).toBeNull();
    });
});

describe('startStreamSource replacement guard', () => {
    it('replaces an ended source on reconnect', async () => {
        const inst = setup();
        const connections = stubStreamFetch();

        await inst.startStreamSource(5, 'stream');
        await flush();
        const first = inst.streamSourceFor(5);

        connections[0].close();
        await flush();
        expect(first!.ended).toBe(true);

        // The Connect gesture: a dead source must not block the restart.
        await inst.startStreamSource(5, 'stream');
        await flush();
        const second = inst.streamSourceFor(5);
        expect(second).not.toBeNull();
        expect(second).not.toBe(first);
        expect(second!.ended).toBe(false);
        expect(connections).toHaveLength(2);

        inst.stopStreamSource(5);
    });

    it('ignores a repeat start while the source is healthy', async () => {
        const inst = setup();
        const connections = stubStreamFetch();

        await inst.startStreamSource(5, 'stream');
        await flush();
        const first = inst.streamSourceFor(5);

        await inst.startStreamSource(5, 'stream');
        expect(inst.streamSourceFor(5)).toBe(first);
        expect(connections).toHaveLength(1);

        inst.stopStreamSource(5);
    });

    it('replaces a source whose acquisition failed without ever ending', async () => {
        // A `MediaStreamSource` denied by getUserMedia has `error` set but
        // `ended === false` — the retry must be admitted anyway, and the
        // failure must clear the session opt-in so Resume reappears.
        const inst = setup();

        inst.markStreamVoidStarted(7);
        await inst.startStreamSource(7, 'camera', undefined, { name: 'NotAllowedError' });
        const first = inst.streamSourceFor(7);
        expect(first!.error).toContain('denied');
        expect(first!.ended).toBe(false);
        expect(first!.status).toBe('disconnected');
        expect(inst.streamSessionStarted.has(7)).toBe(false);

        await inst.startStreamSource(7, 'camera', undefined, { name: 'NotAllowedError' });
        expect(inst.streamSourceFor(7)).not.toBe(first);
    });
});
