import { describe, it, expect, vi } from 'vitest';

// The clone source marker renders via the snapshot-invert overlay path
// (white on dark, black on light, same as the selection marching ants) so
// it stays legible over arbitrary canvas content. `crosshair({ invert })`
// must tag all four arm primitives with FLAG_INVERT_COLOR.

const { fakeApp } = vi.hoisted(() => ({
    fakeApp: { requestFrame: vi.fn() },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../coordinates', () => ({
    canvasToScreen: (cx: number, cy: number) => ({ x: cx, y: cy }),
}));
vi.stubGlobal('window', { devicePixelRatio: 1 });

import { OverlayBuilder } from '../gpu_overlay';
import { KIND_LINE, FLAG_INVERT_COLOR } from '../../tools/selection_helpers';

function pushedPrims(build: (b: OverlayBuilder) => void) {
    const engine = {
        api: { setOverlay: vi.fn(), setCloneOverlay: vi.fn() },
    };
    const b = new OverlayBuilder({} as HTMLCanvasElement);
    build(b);
    b.push(engine as any, 'clone');
    return engine.api.setCloneOverlay.mock.calls[0][0].primitives;
}

describe('OverlayBuilder.crosshair invert option', () => {
    it('tags all four arms with FLAG_INVERT_COLOR', () => {
        const prims = pushedPrims((b) =>
            b.crosshair([10, 10], { invert: true, size: 8, gap: 2, thickness: 1.5 }));
        expect(prims).toHaveLength(4);
        for (const p of prims) {
            expect(p.kind).toBe(KIND_LINE);
            expect(p.flags & FLAG_INVERT_COLOR).toBe(FLAG_INVERT_COLOR);
        }
    });

    it('defaults to plain solid-color arms', () => {
        const prims = pushedPrims((b) =>
            b.crosshair([10, 10], { color: '#f80', size: 6, gap: 3 }));
        expect(prims).toHaveLength(4);
        for (const p of prims) expect(p.flags).toBe(0);
    });
});
