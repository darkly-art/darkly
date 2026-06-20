import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the `app` and `config` modules before importing the module under
// test. The fakes are minimal stand-ins for the Svelte-runic state proxies
// so we don't have to pull the Svelte runtime into a unit test.
const { engine, fakeApp, fakeConfig } = vi.hoisted(() => {
    const engine = {
        post: vi.fn(),
        // Per-kind response: tests set `_pending` / `_picked` before driving
        // the poll. `send` resolves the matching value asynchronously.
        _pending: false,
        _picked: new Uint8Array([0, 0, 0, 0]),
        send: vi.fn((kind: string) => {
            if (kind === 'has_pending_color_pick') return Promise.resolve({ value: engine._pending });
            if (kind === 'last_picked_color') return Promise.resolve({ bytes: engine._picked });
            return Promise.resolve({});
        }),
    };
    const fakeApp = {
        engine,
        activeLayerId: null as number | null,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
    };
    const fakeConfig = {
        // Default value; individual tests override before calling startPick.
        _mode: 'merged' as 'merged' | 'currentLayer',
        get: vi.fn((_key: string) => fakeConfig._mode),
    };
    return { engine, fakeApp, fakeConfig };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));

// Module under test (imported after mocks are registered).
import { startPick, pollPick } from '../color_pick_sync';

/** Find the `engine.post('pick_color', payload)` payload, or undefined. */
function pickColorPayload() {
    const call = engine.post.mock.calls.find(([k]) => k === 'pick_color');
    return call?.[1];
}

/** Drain the promise microtask queue so the chained `send`s settle. */
async function flush() {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
}

function reset() {
    engine.post.mockClear();
    engine.send.mockClear();
    engine._pending = false;
    engine._picked = new Uint8Array([0, 0, 0, 0]);
    fakeApp.activeLayerId = null;
    fakeApp.foreground = { r: 0, g: 0, b: 0, a: 255 };
    fakeConfig._mode = 'merged';
    fakeConfig.get.mockClear();
}

describe('startPick', () => {
    beforeEach(reset);

    it('passes id=-1 in "merged" mode regardless of activeLayerId', () => {
        fakeConfig._mode = 'merged';
        fakeApp.activeLayerId = 42;
        startPick(engine as any, 10, 20);
        expect(pickColorPayload()).toEqual({ x: 10, y: 20, id: -1 });
    });

    it('passes the active layer id in "currentLayer" mode when one is set', () => {
        fakeConfig._mode = 'currentLayer';
        fakeApp.activeLayerId = 42;
        startPick(engine as any, 10, 20);
        expect(pickColorPayload()).toEqual({ x: 10, y: 20, id: 42 });
    });

    it('falls back to -1 in "currentLayer" mode when no layer is active', () => {
        fakeConfig._mode = 'currentLayer';
        fakeApp.activeLayerId = null;
        startPick(engine as any, 10, 20);
        expect(pickColorPayload()).toEqual({ x: 10, y: 20, id: -1 });
    });
});

describe('pollPick', () => {
    beforeEach(reset);

    it('does not overwrite app.foreground when the picked alpha is 0', async () => {
        // Set up an in-flight pick.
        startPick(engine as any, 10, 20);
        // Readback completes with a fully-transparent pixel (outside layer
        // extent, transparent pixel, or unsupported format).
        engine._pending = false;
        engine._picked = new Uint8Array([10, 20, 30, 0]);
        const before = { ...fakeApp.foreground };
        pollPick();
        await flush();
        expect(fakeApp.foreground).toEqual(before);
    });

    it('writes app.foreground when the picked alpha is > 0', async () => {
        startPick(engine as any, 10, 20);
        engine._pending = false;
        engine._picked = new Uint8Array([50, 100, 150, 200]);
        pollPick();
        await flush();
        expect(fakeApp.foreground).toEqual({ r: 50, g: 100, b: 150, a: 200 });
    });

    it('is a no-op when no pick is in flight', async () => {
        // Fresh state — no startPick has been called this test.
        engine._pending = false;
        const before = { ...fakeApp.foreground };
        pollPick();
        await flush();
        expect(fakeApp.foreground).toEqual(before);
        // No query chain fires when nothing is in flight.
        expect(engine.send).not.toHaveBeenCalled();
    });

    it('waits for has_pending_color_pick to clear before committing', async () => {
        startPick(engine as any, 10, 20);
        engine._pending = true;
        const before = { ...fakeApp.foreground };
        pollPick();
        await flush();
        expect(fakeApp.foreground).toEqual(before);
        // The pending query ran, but no last_picked_color read followed.
        expect(engine.send).not.toHaveBeenCalledWith('last_picked_color');
    });
});
