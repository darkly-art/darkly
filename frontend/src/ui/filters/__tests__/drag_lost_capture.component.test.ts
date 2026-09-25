// @vitest-environment jsdom
//
// Regression: a drag whose pointer capture is lost must end, not stick.
//
// Every one of these editors tracks "am I dragging" in a state variable that
// only `pointerup` clears. Pointer capture is not guaranteed to end with a
// `pointerup` on the capturing element: the browser fires `lostpointercapture`
// on its own when the pointer is removed, when the capturing element leaves the
// document, or when another element takes capture. Without a handler for it the
// flag stays set, and the next plain `pointermove` over the control keeps
// dragging with no button held.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

import LevelsEditor from '../LevelsEditor.svelte';
import CurveEditorHarness from './CurveEditorHarness.test.svelte';

// `CurveEditor` observes its own size to lay the grid out; jsdom has no
// ResizeObserver and the callback's measurements are irrelevant here.
class NoopResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
}
vi.stubGlobal('ResizeObserver', NoopResizeObserver);

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
});

/** jsdom implements neither half of the pointer-capture API. */
function stubCapture(el: Element) {
    (el as HTMLElement).setPointerCapture = vi.fn();
    (el as HTMLElement).releasePointerCapture = vi.fn();
}

/** jsdom gives every element a zero-size rect, so a track that maps clientX to
 *  a normalized position would divide by zero. Give it a real width. */
function stubRect(el: Element, width = 256) {
    el.getBoundingClientRect = () =>
        ({ left: 0, top: 0, right: width, bottom: 16, width, height: 16, x: 0, y: 0 }) as DOMRect;
}

function pointer(type: string, init: Partial<PointerEvent> = {}) {
    return new PointerEvent(type, { bubbles: true, pointerId: 1, clientX: 0, clientY: 0, ...init });
}

function render<P extends Record<string, unknown>>(Component: unknown, props: P) {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(
        mount(Component as Parameters<typeof mount>[0], { target, props }) as Record<string, unknown>,
    );
    flushSync();
    return target;
}

describe('LevelsEditor', () => {
    it('stops dragging when pointer capture is lost', () => {
        const oninput = vi.fn();
        const target = render(LevelsEditor, {
            values: [0, 1, 1, 0, 1],
            onchange: vi.fn(),
            oninput,
        });

        const track = target.querySelector('.track')!;
        const handle = target.querySelector('.handle.black')!;
        stubCapture(track);
        stubRect(track);

        handle.dispatchEvent(pointer('pointerdown'));
        flushSync();

        // The browser ends the capture without a pointerup on the track.
        track.dispatchEvent(pointer('lostpointercapture'));
        flushSync();
        oninput.mockClear();

        // A plain move across the track, no button held.
        track.dispatchEvent(pointer('pointermove', { clientX: 200 }));
        flushSync();

        expect(oninput).not.toHaveBeenCalled();
    });
});

describe('CurveEditor', () => {
    it('stops dragging a point when pointer capture is lost', () => {
        const oninput = vi.fn();
        const target = render(CurveEditorHarness, {
            points: [
                [0, 0],
                [1, 1],
            ],
            onchange: vi.fn(),
            oninput,
        });

        const svg = target.querySelector('svg')!;
        stubCapture(svg);
        stubRect(svg, 200);

        const point = target.querySelector('circle')!;
        point.dispatchEvent(pointer('pointerdown'));
        flushSync();

        svg.dispatchEvent(pointer('lostpointercapture'));
        flushSync();
        oninput.mockClear();

        svg.dispatchEvent(pointer('pointermove', { clientX: 100, clientY: 50 }));
        flushSync();

        expect(oninput).not.toHaveBeenCalled();
    });
});
