import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Mock `app` before importing the tools so we never pull in the real engine or
// wasm. Node env, no DOM: each tool must drive the live placement on its own.
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        engine: null as unknown,
        session: null as unknown,
        canvasEl: null as unknown,
        selectLayer: vi.fn(),
        requestFrame: vi.fn(),
        refreshLayerTree: vi.fn(() => Promise.resolve()),
        activeLayerId: null as number | null,
        foreground: { r: 10, g: 20, b: 30, a: 255 },
        consumeForeground() { return this.foreground; },
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => fakeApp }));

import { shapeTools, shapeSession } from '../shape.svelte';

type ShapeToolLike = {
    onActivate(): void;
    onDeactivate(): void;
    claimsPointer(): boolean;
    onPointerDown(e: PointerEvent, cx: number, cy: number): void;
    onPointerMove(e: PointerEvent, cx: number, cy: number): void;
    onPointerUp(e: PointerEvent): Promise<void>;
    onKeyDown(e: KeyboardEvent): boolean;
};
const toolFor = (family: string) =>
    shapeTools.find((t) => t.id === family)!.create(fakeApp as never) as unknown as ShapeToolLike;
const tool = toolFor('polygon');

const ev = (mods: { shiftKey?: boolean; altKey?: boolean; type?: string } = {}) =>
    ({ button: 0, shiftKey: false, altKey: false, type: 'pointerup', ...mods }) as unknown as PointerEvent;

function mockEngine() {
    return withApi({
        send: vi.fn((kind: string) => {
            if (kind === 'commit_shape') return Promise.resolve({ id: 7, object: 0 });
            return Promise.resolve(null);
        }),
        post: vi.fn(),
    });
}

let engine: ReturnType<typeof mockEngine>;

beforeEach(() => {
    engine = mockEngine();
    fakeApp.engine = engine;
    fakeApp.session = engine;
    fakeApp.activeLayerId = null;
    fakeApp.selectLayer.mockClear();
    shapeSession.params = {};
    shapeSession.borderWidth = 0;
    shapeSession.borderColor = [0, 0, 0, 255];
});

const posted = (kind: string) =>
    engine.post.mock.calls.filter((c: unknown[]) => c[0] === kind).map((c: unknown[]) => c[1]);
const sent = (kind: string) => engine.send.mock.calls.filter((c: unknown[]) => c[0] === kind);

async function drag(from: [number, number], to: [number, number], mods = {}) {
    tool.onPointerDown(ev(mods), from[0], from[1]);
    tool.onPointerMove(ev(mods), to[0], to[1]);
    await tool.onPointerUp(ev(mods));
}

describe('shape tools', () => {
    it('are one vector tool per family, clustered, with no options bar', () => {
        expect(shapeTools.map((t) => t.id)).toEqual(['rectangle', 'ellipse', 'polygon']);
        for (const t of shapeTools) {
            expect(t.cluster).toBe('shape');
            // The color sampler's modifier chord (Alt+drag under the photoshop
            // overlay) arms over any `paint`-group tool and swallows its
            // pointer moves, so Alt-from-centre only works outside that group.
            expect(t.group).toBe('vector');
            expect(t.optionsComponent).toBeUndefined();
        }
        expect(tool.claimsPointer()).toBe(true);
    });

    it('activation warms the vector renderer', () => {
        tool.onActivate();
        expect(engine.post).toHaveBeenCalledWith('warm_vector_renderer');
    });

    it("a drag begins its family's shape live, follows the pointer, then commits and selects it", async () => {
        fakeApp.activeLayerId = 3;
        shapeSession.params = { polygon: { points: 6 } };
        tool.onPointerDown(ev(), 20, 30);
        tool.onPointerMove(ev(), 22, 31);
        expect(posted('begin_shape')).toEqual([]);
        tool.onPointerMove(ev(), 60, 60);
        tool.onPointerMove(ev(), 120, 90);
        expect(posted('begin_shape')).toEqual([
            {
                family: 'polygon',
                params: { points: 6 },
                x: 20,
                y: 30,
                width: 40,
                height: 30,
                fill: [10, 20, 30, 255],
                stroke: { width: 0, color: [0, 0, 0, 255] },
                anchor: 3,
            },
        ]);
        expect(posted('update_shape')).toEqual([{ x: 20, y: 30, width: 100, height: 60 }]);
        await tool.onPointerUp(ev());
        expect(sent('commit_shape')).toHaveLength(1);
        expect(fakeApp.selectLayer).toHaveBeenCalledWith(7);
    });

    it('each family tool places its own family', async () => {
        const ellipse = toolFor('ellipse');
        ellipse.onPointerDown(ev(), 0, 0);
        ellipse.onPointerMove(ev(), 40, 40);
        expect(posted('begin_shape')[0]).toMatchObject({ family: 'ellipse' });
    });

    it('carries the remembered border', async () => {
        shapeSession.borderWidth = 3;
        shapeSession.borderColor = [1, 2, 3, 255];
        await drag([0, 0], [40, 40]);
        expect(posted('begin_shape')[0]).toMatchObject({ stroke: { width: 3, color: [1, 2, 3, 255] } });
    });

    // Alt-held moves must reach the tool and centre the box on the press point.
    it('an Alt-held drag places a box centred on the press point', async () => {
        await drag([50, 50], [80, 70], { altKey: true });
        expect(posted('begin_shape')[0]).toMatchObject({ x: 20, y: 30, width: 60, height: 40 });
    });

    it('a click places nothing', async () => {
        await drag([50, 50], [51, 52]);
        expect(posted('begin_shape')).toEqual([]);
        expect(sent('commit_shape')).toEqual([]);
    });

    it('releasing after shrinking below the threshold still places what is on screen', async () => {
        tool.onPointerDown(ev(), 0, 0);
        tool.onPointerMove(ev(), 40, 40);
        tool.onPointerMove(ev(), 1, 1);
        await tool.onPointerUp(ev());
        expect(sent('commit_shape')).toHaveLength(1);
        expect(posted('cancel_shape')).toEqual([]);
    });

    it('Escape after the shape appeared cancels it', async () => {
        tool.onPointerDown(ev(), 0, 0);
        tool.onPointerMove(ev(), 50, 50);
        expect(tool.onKeyDown({ key: 'Escape' } as KeyboardEvent)).toBe(true);
        expect(posted('cancel_shape')).toHaveLength(1);
        await tool.onPointerUp(ev());
        expect(sent('commit_shape')).toEqual([]);
        expect(fakeApp.selectLayer).not.toHaveBeenCalled();
    });

    it('switching tool mid-drag cancels the shape', () => {
        tool.onPointerDown(ev(), 0, 0);
        tool.onPointerMove(ev(), 50, 50);
        tool.onDeactivate();
        expect(posted('cancel_shape')).toHaveLength(1);
    });

    it('a lost pen (pointercancel) cancels rather than places', async () => {
        tool.onPointerDown(ev(), 0, 0);
        tool.onPointerMove(ev(), 50, 50);
        await tool.onPointerUp(ev({ type: 'pointercancel' }));
        expect(posted('cancel_shape')).toHaveLength(1);
        expect(sent('commit_shape')).toEqual([]);
    });
});
