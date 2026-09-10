import { describe, it, expect, vi, beforeEach } from 'vitest';

// Two-finger touch rotation is the riskiest seam of the canvas-rotation snap:
// it drives `app.rotation` from a per-gesture *unsnapped* accumulator so that
// small in-zone deltas aren't swallowed by the cardinal dead-zone. We mock the
// `app` view-state and `config` so we can feed `nav` a synthetic touch sequence
// and read back the snapped rotation directly.
const { fakeApp } = vi.hoisted(() => {
    const fakeApp = {
        panX: 0, panY: 0, zoom: 1, rotation: 0,
        toolCursor: null as string | null,
        requestFrame: () => {},
    };
    return { fakeApp };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({
    config: {
        get(key: string): unknown {
            if (key === 'nav.rotateDetent') return true;
            if (key === 'input.fingerPainting') return true;
            return undefined;
        },
    },
}));

import { nav } from '../navigation.svelte';

const DEG = Math.PI / 180;
const CARDINAL = 45 * DEG;

function canvasEl(): HTMLCanvasElement {
    return {
        getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
    } as unknown as HTMLCanvasElement;
}

/** A touch pointer event fake. */
function ptr(id: number, x: number, y: number): PointerEvent {
    return { pointerId: id, clientX: x, clientY: y } as PointerEvent;
}

/** Finger-2 position 100px from finger-1 (at origin) at the given angle. */
function finger2(angleDeg: number): [number, number] {
    return [100 * Math.cos(angleDeg * DEG), 100 * Math.sin(angleDeg * DEG)];
}

beforeEach(() => {
    fakeApp.panX = 0; fakeApp.panY = 0; fakeApp.zoom = 1; fakeApp.rotation = 0;
    fakeApp.toolCursor = null;
    nav.spaceHeld = false;
    nav.mode = 'none';
    // Drain any lingering touch state from a previous test.
    nav.onTouchPointerUp(ptr(1, 0, 0));
    nav.onTouchPointerUp(ptr(2, 0, 0));
});

describe('canvasCursor precedence', () => {
    it('nav owns the cursor while navigating, even when a tool hides it', () => {
        // Brush hides the native cursor (draws its own ring). Holding the nav
        // trigger to drag must still surface the grab/rotate cursor, not 'none'.
        fakeApp.toolCursor = 'none';
        nav.spaceHeld = true;

        nav.mode = 'pan';
        expect(nav.canvasCursor).toBe('grabbing');
        nav.mode = 'rotate';
        expect(nav.canvasCursor).not.toBe('none'); // the custom rotate cursor
        nav.mode = 'none';
        expect(nav.canvasCursor).toBe('grab'); // held, not yet dragging
    });

    it('off the nav path the tool cursor wins, falling back to nav idle', () => {
        nav.spaceHeld = false;
        fakeApp.toolCursor = 'none';
        expect(nav.canvasCursor).toBe('none');
        fakeApp.toolCursor = null;
        expect(nav.canvasCursor).toBe('crosshair');
    });
});

describe('two-finger canvas rotation snapping', () => {
    it('parks at the 45° mark while inside the ±2° band, then releases', () => {
        // Seed just shy of 45° so the gesture starts inside the detent band.
        fakeApp.rotation = 43 * DEG;
        const el = canvasEl();
        nav.onTouchPointerDown(ptr(1, 0, 0));
        nav.onTouchPointerDown(ptr(2, 100, 0)); // seeds rawRotation = 43°

        // Each move rotates finger 2 by −1°, which adds +1° to the raw rotation.
        const display: number[] = [];
        for (let k = 1; k <= 5; k++) {
            const [x, y] = finger2(-k);
            nav.onTouchPointerMove(ptr(2, x, y), el);
            display.push(fakeApp.rotation);
        }

        // raw 44° and 46° sit within ±2° of 45° → parked exactly at 45°.
        expect(display[0]).toBeCloseTo(CARDINAL, 6); // raw 44°
        expect(display[2]).toBeCloseTo(CARDINAL, 6); // raw 46°, still parked
        // raw 48° escapes the band → free rotation, NOT stuck at 45°.
        expect(display[4]).toBeCloseTo(48 * DEG, 6);
    });

    it('does not jump when a lifted finger returns (2→1→2)', () => {
        fakeApp.rotation = 0;
        const el = canvasEl();
        nav.onTouchPointerDown(ptr(1, 0, 0));
        nav.onTouchPointerDown(ptr(2, 100, 0));

        // Rotate well clear of any cardinal so we read free rotation.
        nav.onTouchPointerMove(ptr(2, ...finger2(-20)), el);
        const before = fakeApp.rotation;
        expect(before).toBeCloseTo(20 * DEG, 6);

        // Drop finger 2, then bring it back at a completely different position.
        nav.onTouchPointerUp(ptr(2, 0, 0));
        nav.onTouchPointerDown(ptr(2, 0, 100)); // returns at angle +90°, far away

        // A tiny subsequent move applies only its own delta: no jump from the
        // reposition, because prevAngle re-snapshots (from the 90° return) and
        // rawRotation is not reseeded. Move finger 2 from 90° to 89°.
        nav.onTouchPointerMove(ptr(2, 100 * Math.cos(89 * DEG), 100 * Math.sin(89 * DEG)), el);
        expect(fakeApp.rotation).toBeCloseTo(before + 1 * DEG, 4);
    });
});
