import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the module's two dependencies before importing it, so its top-level
// `import { app }` / `toast` chain resolves to fakes (same pattern as
// clipboard.test.ts).
const { engine, fakeApp, fakeToast } = vi.hoisted(() => {
    const engine = {
        api: {
            placeSmartObject: vi.fn().mockResolvedValue({ id: 7 }),
            pasteImage: vi.fn().mockResolvedValue({ id: 9 }),
        },
    };
    const fakeApp = {
        engine,
        activeLayerId: 42 as number | null,
        selectLayer: vi.fn(),
        requestFrame: vi.fn(),
        refreshLayerTree: vi.fn().mockResolvedValue(undefined),
    };
    const fakeToast = { show: vi.fn() };
    return { engine, fakeApp, fakeToast };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../state/toast.svelte', () => ({ toast: fakeToast }));

import { decodeToRgba, placeSmartObjectFromBlob } from '../place_smart_object';
// `handleDroppedFile` lives in `actions/index.ts`, which pulls the whole action
// registry (and every tool's options component) in behind it. Imported at the
// top level, not lazily inside a test: `vi.mock` is hoisted above this either
// way, and paying that compile inside a test body puts it under the 5s test
// timeout, where a loaded machine intermittently blows through it — and the
// call it left in flight then lands during the *next* test, tripping that one
// too. Collection has no such deadline.
import { handleDroppedFile } from '../index';

/** Stub the decode pipeline. Vitest runs in node: there is no
 *  `createImageBitmap` / `OffscreenCanvas`, so both are faked with plain
 *  objects that record what they were asked for. */
function stubDecode(width: number, height: number) {
    const created: Array<Record<string, unknown> | undefined> = [];
    vi.stubGlobal(
        'createImageBitmap',
        vi.fn(async (_src: unknown, opts?: Record<string, unknown>) => {
            created.push(opts);
            const w = (opts?.resizeWidth as number) ?? width;
            const h = (opts?.resizeHeight as number) ?? height;
            return { width: w, height: h, close: vi.fn() };
        }),
    );
    vi.stubGlobal(
        'OffscreenCanvas',
        class {
            constructor(
                public width: number,
                public height: number,
            ) {}
            getContext() {
                return {
                    drawImage: vi.fn(),
                    getImageData: (_x: number, _y: number, w: number, h: number) => ({
                        data: new Uint8ClampedArray(w * h * 4),
                    }),
                };
            }
        },
    );
    return created;
}

beforeEach(() => {
    engine.api.placeSmartObject.mockClear();
    engine.api.pasteImage.mockClear();
    fakeApp.selectLayer.mockClear();
    fakeToast.show.mockClear();
    fakeApp.activeLayerId = 42;
    vi.unstubAllGlobals();
});

describe('decodeToRgba', () => {
    it('returns the image at its own size when within the cap', async () => {
        const created = stubDecode(800, 600);
        const out = await decodeToRgba({} as Blob);
        expect(out).toMatchObject({ width: 800, height: 600 });
        expect(out!.rgba.length).toBe(800 * 600 * 4);
        // One decode, no resize pass.
        expect(created).toEqual([undefined]);
    });

    it('downscales an oversized image to the cap, preserving aspect', async () => {
        const created = stubDecode(8192, 4096);
        const out = await decodeToRgba({} as Blob);
        // 8192 → 4096 is a halving, so the short axis halves too.
        expect(out).toMatchObject({ width: 4096, height: 2048 });
        expect(created[1]).toMatchObject({
            resizeWidth: 4096,
            resizeHeight: 2048,
            resizeQuality: 'high',
        });
        expect(fakeToast.show).toHaveBeenCalledWith(
            'info',
            expect.stringContaining('4096'),
        );
    });

    it('returns null rather than throwing when the decode fails', async () => {
        vi.stubGlobal(
            'createImageBitmap',
            vi.fn(async () => {
                throw new Error('not an image');
            }),
        );
        expect(await decodeToRgba({} as Blob)).toBeNull();
    });
});

describe('placeSmartObjectFromBlob', () => {
    it('sends the decoded dimensions and selects the new layer', async () => {
        stubDecode(320, 240);
        const id = await placeSmartObjectFromBlob({} as Blob, 'logo.png');

        expect(id).toBe(7);
        expect(engine.api.placeSmartObject).toHaveBeenCalledTimes(1);
        const [req, bytes] = engine.api.placeSmartObject.mock.calls[0];
        expect(req).toEqual({ width: 320, height: 240, active_layer_id: 42 });
        expect(bytes.length).toBe(320 * 240 * 4);
        expect(fakeApp.selectLayer).toHaveBeenCalledWith(7);
    });

    it('passes -1 as the anchor when no layer is active', async () => {
        stubDecode(10, 10);
        fakeApp.activeLayerId = null;
        await placeSmartObjectFromBlob({} as Blob, 'logo.png');
        expect(engine.api.placeSmartObject.mock.calls[0][0].active_layer_id).toBe(-1);
    });

    it('toasts and does not call the engine when the image cannot be decoded', async () => {
        vi.stubGlobal(
            'createImageBitmap',
            vi.fn(async () => {
                throw new Error('nope');
            }),
        );
        const id = await placeSmartObjectFromBlob({} as Blob, 'broken.png');

        expect(id).toBe(-1);
        expect(engine.api.placeSmartObject).not.toHaveBeenCalled();
        expect(fakeToast.show).toHaveBeenCalledWith(
            'error',
            expect.stringContaining('broken.png'),
        );
    });

    it('never falls back to a raster paste', async () => {
        stubDecode(64, 64);
        await placeSmartObjectFromBlob({} as Blob, 'logo.png');
        expect(engine.api.pasteImage).not.toHaveBeenCalled();
    });
});

// ---------------------------------------------------------------------------
// Drop routing, through the real `handleDroppedFile` with the same fakes.
// ---------------------------------------------------------------------------

describe('drop routing', () => {
    async function drop(bytes: Uint8Array, altKey: boolean) {
        const file = {
            name: 'logo.png',
            arrayBuffer: async () => bytes.buffer,
        } as unknown as File;
        await handleDroppedFile(file, altKey);
    }

    /** PNG magic bytes, so `detectKind` routes this as an image. */
    const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0]);

    it('pastes a plain image drop as a raster layer', async () => {
        stubDecode(32, 32);
        await drop(PNG, false);
        expect(engine.api.pasteImage).toHaveBeenCalledTimes(1);
        expect(engine.api.placeSmartObject).not.toHaveBeenCalled();
    });

    it('places an Alt-held image drop as a smart object', async () => {
        stubDecode(32, 32);
        await drop(PNG, true);
        expect(engine.api.placeSmartObject).toHaveBeenCalledTimes(1);
        expect(engine.api.pasteImage).not.toHaveBeenCalled();
    });
});
