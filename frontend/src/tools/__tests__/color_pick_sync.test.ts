import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the `app` and `config` modules before importing the module under
// test. The fakes are minimal stand-ins for the Svelte-runic state proxies
// so we don't have to pull the Svelte runtime into a unit test.
const { handle, fakeApp, fakeConfig } = vi.hoisted(() => {
    const handle = {
        pick_color: vi.fn(),
        has_pending_color_pick: vi.fn().mockReturnValue(false),
        last_picked_color: vi.fn().mockReturnValue(new Uint8Array([0, 0, 0, 0])),
    };
    const fakeApp = {
        handle,
        activeLayerId: null as number | null,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
    };
    const fakeConfig = {
        // Default value; individual tests override before calling startPick.
        _mode: 'merged' as 'merged' | 'currentLayer',
        get: vi.fn((_key: string) => fakeConfig._mode),
    };
    return { handle, fakeApp, fakeConfig };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));

// Module under test (imported after mocks are registered).
import { startPick, pollPick } from '../color_pick_sync';

function reset() {
    handle.pick_color.mockClear();
    handle.has_pending_color_pick.mockClear();
    handle.has_pending_color_pick.mockReturnValue(false);
    handle.last_picked_color.mockClear();
    handle.last_picked_color.mockReturnValue(new Uint8Array([0, 0, 0, 0]));
    fakeApp.activeLayerId = null;
    fakeApp.foreground = { r: 0, g: 0, b: 0, a: 255 };
    fakeConfig._mode = 'merged';
    fakeConfig.get.mockClear();
}

describe('startPick', () => {
    beforeEach(reset);

    it('passes layer_id=-1 in "merged" mode regardless of activeLayerId', () => {
        fakeConfig._mode = 'merged';
        fakeApp.activeLayerId = 42;
        startPick(handle as any, 10, 20);
        expect(handle.pick_color).toHaveBeenCalledWith(10, 20, -1);
    });

    it('passes the active layer id in "currentLayer" mode when one is set', () => {
        fakeConfig._mode = 'currentLayer';
        fakeApp.activeLayerId = 42;
        startPick(handle as any, 10, 20);
        expect(handle.pick_color).toHaveBeenCalledWith(10, 20, 42);
    });

    it('falls back to -1 in "currentLayer" mode when no layer is active', () => {
        fakeConfig._mode = 'currentLayer';
        fakeApp.activeLayerId = null;
        startPick(handle as any, 10, 20);
        expect(handle.pick_color).toHaveBeenCalledWith(10, 20, -1);
    });
});

describe('pollPick', () => {
    beforeEach(reset);

    it('does not overwrite app.foreground when the picked alpha is 0', () => {
        // Set up an in-flight pick.
        startPick(handle as any, 10, 20);
        // Readback completes with a fully-transparent pixel (outside layer
        // extent, transparent pixel, or unsupported format).
        handle.has_pending_color_pick.mockReturnValue(false);
        handle.last_picked_color.mockReturnValue(new Uint8Array([10, 20, 30, 0]));
        const before = { ...fakeApp.foreground };
        pollPick();
        expect(fakeApp.foreground).toEqual(before);
    });

    it('writes app.foreground when the picked alpha is > 0', () => {
        startPick(handle as any, 10, 20);
        handle.has_pending_color_pick.mockReturnValue(false);
        handle.last_picked_color.mockReturnValue(new Uint8Array([50, 100, 150, 200]));
        pollPick();
        expect(fakeApp.foreground).toEqual({ r: 50, g: 100, b: 150, a: 200 });
    });

    it('is a no-op when no pick is in flight', () => {
        // Fresh state — no startPick has been called this test (reset clears
        // module flag indirectly via the alpha-zero / writes tests above
        // running first wouldn't matter because pollPick clears its own
        // waitingForPick on success). Force the not-pending state and
        // confirm we don't read or write anything.
        handle.has_pending_color_pick.mockReturnValue(false);
        const before = { ...fakeApp.foreground };
        pollPick();
        expect(fakeApp.foreground).toEqual(before);
        expect(handle.last_picked_color).not.toHaveBeenCalled();
    });

    it('waits for has_pending_color_pick to clear before committing', () => {
        startPick(handle as any, 10, 20);
        handle.has_pending_color_pick.mockReturnValue(true);
        const before = { ...fakeApp.foreground };
        pollPick();
        expect(fakeApp.foreground).toEqual(before);
        expect(handle.last_picked_color).not.toHaveBeenCalled();
    });
});
