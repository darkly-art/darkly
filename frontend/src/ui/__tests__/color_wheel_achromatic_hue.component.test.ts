// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { Color } from '../../state/app.svelte';
import ColorWheelHarness from './ColorWheelHarness.test.svelte';
import { pointForHue, pointForSv, wheelGeometry } from '../color/wheel_model';

// jsdom has no 2D canvas and no pointer capture; the wheel needs both to
// mount and to take a click. Neither affects the color math under test.
beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () =>
            ({
                createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
                putImageData() {},
            }) as unknown as ReturnType<HTMLCanvasElement['getContext']>,
    );
    Element.prototype.setPointerCapture = () => {};
    Element.prototype.hasPointerCapture = () => false;
});

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    vi.restoreAllMocks();
});

const SIZE = 200;
// The same proportions the component uses, from their one home.
const g = wheelGeometry(SIZE);

/** Mounts the wheel inside a host that, like every real host, feeds the
 *  wheel's own output straight back into its `value`: the round trip through
 *  RGB that the wheel must survive. */
function render(initial: Color) {
    const target = document.createElement('div');
    document.body.append(target);
    const host = { value: initial };
    mounted.push(
        mount(ColorWheelHarness, {
            target,
            props: { initial, onvalue: (c: Color) => (host.value = c), size: SIZE },
        }) as Record<string, unknown>,
    );
    const wheel = target.querySelector('.wheel')!;
    wheel.getBoundingClientRect = () =>
        ({ left: 0, top: 0, right: SIZE, bottom: SIZE, width: SIZE, height: SIZE, x: 0, y: 0, toJSON() {} }) as DOMRect;
    return { wheel, host };
}

function pointerDown(el: Element, x: number, y: number) {
    el.dispatchEvent(new PointerEvent('pointerdown', { clientX: x, clientY: y, pointerId: 1, button: 0, bubbles: true }));
}

describe('choosing a hue on an achromatic foreground', () => {
    it('hue_chosen_on_black_survives_the_next_saturation_click', () => {
        const { wheel, host } = render({ r: 0, g: 0, b: 0, a: 255 });
        flushSync();

        // Pick hue 120 (green). On black this writes black back to the host,
        // so the color itself cannot show whether the hue stuck.
        const ring = pointForHue(g, 120);
        pointerDown(wheel, ring.x, ring.y);
        // Load-bearing: the host's write flows back into the wheel's `value`
        // in an effect. Without flushing, the second click runs before that
        // effect, and a wheel that re-derives its hue from the bytes would
        // pass falsely.
        flushSync();

        // Full saturation and value: the result must be the chosen hue.
        const corner = pointForSv(g, 120, 1, 1);
        pointerDown(wheel, corner.x, corner.y);
        flushSync();

        expect(host.value).toEqual({ r: 0, g: 255, b: 0, a: 255 });
    });

    it('an_external_gray_keeps_the_wheel_hue', () => {
        const { wheel, host } = render({ r: 255, g: 0, b: 0, a: 255 });
        flushSync();
        const ring = pointForHue(g, 240);
        pointerDown(wheel, ring.x, ring.y);
        flushSync();
        expect(host.value).toEqual({ r: 0, g: 0, b: 255, a: 255 });

        // The host resets to gray from outside (eyedropper, reset, undo).
        (mounted[0] as unknown as { setValue: (c: Color) => void }).setValue({ r: 128, g: 128, b: 128, a: 255 });
        flushSync();

        const corner = pointForSv(g, 240, 1, 1);
        pointerDown(wheel, corner.x, corner.y);
        flushSync();
        expect(host.value).toEqual({ r: 0, g: 0, b: 255, a: 255 });
    });
});
