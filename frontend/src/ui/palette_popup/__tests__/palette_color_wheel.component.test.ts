// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The wheel the spectrum leaf summons edits the focused instance's foreground
// and reads the color prefs through the config store, which is WASM-backed in
// production.
vi.mock('../../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: (await import('../../../__tests__/fakeConfig.svelte')).fakeConfig,
    tooltipForAction: (label: string) => label,
}));

import { fakeConfig } from '../../../__tests__/fakeConfig.svelte';
import { DarklyInstance, setActiveInstance } from '../../../state/app.svelte';
import { palettePopup } from '../../../state/palettePopup.svelte';
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';
import type { WheelTree } from '../model';
import PalettePopup from '../PalettePopup.svelte';

const mounted: Array<Record<string, unknown>> = [];

beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () =>
            ({
                createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
                putImageData() {},
            }) as unknown as ReturnType<HTMLCanvasElement['getContext']>,
    );
    setActiveInstance(new DarklyInstance());
    fakeConfig.reset();
});
afterEach(() => {
    palettePopup.closeColorWheel();
    palettePopup.cancel();
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    setActiveInstance(null);
    vi.restoreAllMocks();
});

function render() {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(PalettePopup, { target }) as Record<string, unknown>);
    flushSync();
    return target;
}

/** A wheel holding one spectrum leaf, and nothing that carries a name.
 *
 *  Unnamed deliberately: a named fan is measured off hidden `<text>` twins,
 *  and jsdom lays out no SVG text. Nothing here needs a name, so nothing here
 *  measures one. */
function spectrumTree(): WheelTree {
    return {
        sections: [{
            a0: Math.PI / 6,
            span: (2 * Math.PI) / 3,
            nodes: [{
                kind: 'leaf',
                id: 'color:spectrum',
                label: 'Color wheel',
                visual: { kind: 'spectrum' },
                palette: NEUTRAL_PALETTE,
                select() {},
            }],
        }],
    };
}

/** Drive the gesture straight into its engaged state, which is what the
 *  drag-chord dispatcher does through `open` / `move` in the running app. */
function engage(tree: WheelTree) {
    palettePopup.tree = tree;
    palettePopup.state = {
        kind: 'engaged',
        pointerId: 1,
        center: { x: 300, y: 300 },
        cursor: { x: 300, y: 300 },
        path: [],
        highlight: { kind: 'hub' },
    };
    flushSync();
}

describe('palette popup color wheel', () => {
    it('is closed until the spectrum leaf asks for it', () => {
        const target = render();
        expect(target.querySelector('.color-popup')).toBeNull();
    });

    it('opens at the point it was summoned from and outlives the gesture', () => {
        const target = render();

        palettePopup.openColorWheel({ x: 100, y: 120 });
        flushSync();

        const popup = target.querySelector('.color-popup');
        expect(popup).not.toBeNull();
        // Its own dismissal scope, so it and the swatches' wheel never close
        // each other.
        expect(popup!.getAttribute('data-keep-open')).toBe('palette-color-wheel');
        expect(popup!.querySelector('.wheel')).not.toBeNull();
        // No gesture is running: the wheel is mounted outside the overlay that
        // the pen lifting unmounts.
        expect(palettePopup.isOpen).toBe(false);
    });

    it('closes on a press outside it', () => {
        const target = render();
        palettePopup.openColorWheel({ x: 100, y: 120 });
        flushSync();

        document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        flushSync();

        expect(target.querySelector('.color-popup')).toBeNull();
        expect(palettePopup.colorWheelAt).toBeNull();
    });

    it('paints the spectrum sector with the hue ramp and gives it no pack rim', () => {
        const target = render();
        engage(spectrumTree());

        const sectors = target.querySelectorAll('path.sector');
        expect(sectors).toHaveLength(1);
        // jsdom re-serializes the url() with quotes, so match on the id.
        const style = sectors[0].getAttribute('style') ?? '';
        expect(style).toMatch(/fill: url\(["']?#palette-hue-0/);
        expect(style).toMatch(/stroke: url\(["']?#palette-hue-0/);
        // A painted sector states no pack, so it keeps the full stroke width
        // rather than giving some up to a rim.
        expect(sectors[0].classList.contains('rimmed')).toBe(false);
        expect(target.querySelector('#palette-hue-0')).not.toBeNull();
        expect(target.querySelectorAll('#palette-hue-0 stop')).toHaveLength(7);
    });
});
