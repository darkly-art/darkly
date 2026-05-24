import { describe, it, expect, vi, beforeEach } from 'vitest';

// `vi.hoisted` lets the mock factory and the test bodies share the same
// spy. `vi.mock` is hoisted above the top-level `import`s, so a plain
// `const` would not be in scope inside the factory.
const { downloadFile } = vi.hoisted(() => ({ downloadFile: vi.fn() }));

vi.mock('../downloadFile', () => ({ downloadFile }));
// strokeRecorder imports `app` from `state/app.svelte` for the dimensions
// helper. The recorder methods themselves never touch `app` — `app.handle`
// is consulted only by `currentCanvasDimensions`, which the tests don't
// exercise. Stub it so the import resolves without pulling in Svelte runes.
vi.mock('../../state/app.svelte', () => ({ app: { handle: null, docW: 0, docH: 0 } }));

import { strokeRecorder, type RecordedEventParams } from '../strokeRecorder';

function makeEvent(overrides: Partial<RecordedEventParams> = {}): RecordedEventParams {
    return {
        x: 100,
        y: 200,
        pressure: 0.5,
        x_tilt: 0.1,
        y_tilt: -0.2,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0,
        cr: 0,
        cg: 0,
        cb: 0,
        ca: 1,
        ...overrides,
    };
}

function setUrlSearch(search: string): void {
    // Vitest's node env exposes a writable `globalThis.location` shim with
    // an empty `search`. Just mutate it — defineProperty would complain
    // about non-configurable descriptors on some Node versions.
    (globalThis as any).location = { search };
}

describe('strokeRecorder', () => {
    beforeEach(() => {
        downloadFile.mockClear();
    });

    it('records a full begin/add/end sequence and triggers exactly one download with the expected schema', () => {
        setUrlSearch('?_RECORD_STROKES=1');
        strokeRecorder.init();
        expect(strokeRecorder.isEnabled).toBe(true);

        strokeRecorder.beginStroke(1920, 1080, makeEvent({ x: 1, y: 2, time_ms: 1000 }));
        strokeRecorder.addEvent(makeEvent({ x: 3, y: 4, time_ms: 1016 }));
        strokeRecorder.addEvent(makeEvent({ x: 5, y: 6, time_ms: 1032 }));
        strokeRecorder.addEvent(makeEvent({ x: 7, y: 8, time_ms: 1048 }));
        strokeRecorder.endStroke();

        expect(downloadFile).toHaveBeenCalledTimes(1);
        const [body, filename, mime] = downloadFile.mock.calls[0];
        expect(filename).toMatch(/^darkly-stroke-.*\.json$/);
        expect(mime).toBe('application/json');

        const parsed = JSON.parse(body as string);
        expect(parsed.version).toBe(1);
        expect(parsed.canvas_width).toBe(1920);
        expect(parsed.canvas_height).toBe(1080);
        expect(parsed.events).toHaveLength(4);
        expect(parsed.events[0]).toMatchObject({ x: 1, y: 2, time_ms: 1000 });
        expect(parsed.events[3]).toMatchObject({ x: 7, y: 8, time_ms: 1048 });
        expect(typeof parsed.recorded_at).toBe('string');
    });

    it('is a no-op when ?_RECORD_STROKES is absent', () => {
        setUrlSearch('');
        strokeRecorder.init();
        expect(strokeRecorder.isEnabled).toBe(false);

        strokeRecorder.beginStroke(800, 600, makeEvent());
        strokeRecorder.addEvent(makeEvent());
        strokeRecorder.endStroke();

        expect(downloadFile).not.toHaveBeenCalled();
    });

    it('clears its buffer between strokes so each download is exactly one stroke', () => {
        setUrlSearch('?_RECORD_STROKES=1');
        strokeRecorder.init();

        strokeRecorder.beginStroke(100, 100, makeEvent({ x: 1, time_ms: 0 }));
        strokeRecorder.addEvent(makeEvent({ x: 2, time_ms: 16 }));
        strokeRecorder.endStroke();

        strokeRecorder.beginStroke(100, 100, makeEvent({ x: 10, time_ms: 0 }));
        strokeRecorder.addEvent(makeEvent({ x: 20, time_ms: 16 }));
        strokeRecorder.addEvent(makeEvent({ x: 30, time_ms: 32 }));
        strokeRecorder.endStroke();

        expect(downloadFile).toHaveBeenCalledTimes(2);
        const first = JSON.parse(downloadFile.mock.calls[0][0] as string);
        const second = JSON.parse(downloadFile.mock.calls[1][0] as string);
        expect(first.events.map((e: RecordedEventParams) => e.x)).toEqual([1, 2]);
        expect(second.events.map((e: RecordedEventParams) => e.x)).toEqual([10, 20, 30]);
    });

    it('addEvent before beginStroke is ignored', () => {
        setUrlSearch('?_RECORD_STROKES=1');
        strokeRecorder.init();

        strokeRecorder.addEvent(makeEvent());
        strokeRecorder.endStroke();

        expect(downloadFile).not.toHaveBeenCalled();
    });
});
