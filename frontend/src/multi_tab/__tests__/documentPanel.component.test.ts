// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The tool strip and the tool-options bar resolve tooltips through the config
// store, and the strip's placement is a pref read from it.
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: (await import('../../__tests__/fakeConfig.svelte')).fakeConfig,
    tooltipForAction: (label: string) => label,
}));

// `ToolOptionsBar` imports the brush graph, which transitively pulls in the
// brush library and recents modules; those touch persistent storage at import
// time. Only `fullscreen` is read here.
vi.mock('../../state/brush_graph.svelte', () => ({
    brushGraph: { fullscreen: false },
}));

import { fakeConfig } from '../../__tests__/fakeConfig.svelte';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import { menuBar } from '../../state/menuBar.svelte';
import { toolStripPlacement } from '../../ui/tool_strip/placement.svelte';
import { canvasSlot } from '../canvasSlot.svelte';
import DocumentPanel from '../DocumentPanel.svelte';

let inst: DarklyInstance;
const mounted: Array<Record<string, unknown>> = [];

beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () =>
            ({
                createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
                putImageData() {},
            }) as unknown as ReturnType<HTMLCanvasElement['getContext']>,
    );
    inst = new DarklyInstance();
    setActiveInstance(inst);
    fakeConfig.reset();
    toolStripPlacement.override = null;
    canvasSlot.rect = null;
});

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    setActiveInstance(null);
    if (menuBar.pinned) menuBar.toggle();
    vi.restoreAllMocks();
});

function render() {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(DocumentPanel, { target }) as Record<string, unknown>);
    flushSync();
    return target;
}

// Structure and inline styles only: Svelte's scoped CSS is not reliably
// applied under vitest, so `getComputedStyle` proves nothing here. The
// change's real risk, that the strip paints above the WebGPU canvases, is
// invisible to jsdom at any number of assertions (it computes no layout and
// `CanvasOverlay` is not mounted). So is which way the strip and its flyouts
// actually face, since that is entirely the per-edge CSS table. The comments in
// `styles/tool-strip-dock.css` are the defence for both.
describe('document panel layout', () => {
    it('tool_strip_floats_inside_the_canvas_region', () => {
        const target = render();

        expect(target.querySelector('.canvas-region .toolbar')).not.toBeNull();
        // Not a column beside the canvas: that is the whole point of the change.
        expect(target.querySelector('.document-panel > .toolbar')).toBeNull();
    });

    it('hamburger_sits_in_the_top_bar_outside_the_scrolling_tab_strip', () => {
        const target = render();

        const hamburger = target.querySelector('.doc-top .hamburger-btn');
        expect(hamburger).not.toBeNull();
        // Inside `.tab-strip` it would scroll away with the tabs and its
        // dropdown would be clipped by the strip's `overflow-x: auto`.
        expect(hamburger!.closest('.tab-strip')).toBeNull();
    });

    it('pinning_the_menu_bar_removes_the_hamburger_from_the_top_bar', () => {
        const target = render();
        expect(target.querySelector('.doc-top .hamburger-btn')).not.toBeNull();

        menuBar.toggle();
        flushSync();

        expect(target.querySelector('.doc-top .hamburger-btn')).toBeNull();
    });

    it('strip_and_canvas_region_carry_the_edge_the_placement_reports', () => {
        fakeConfig.set('ui.toolStrip.edge', 'bottom');
        const target = render();

        expect(target.querySelector('.toolbar')!.getAttribute('data-edge')).toBe('bottom');
        // Passed through so the CSS table can clip the tuck axis; DocumentPanel
        // never asks the edge what it is.
        expect(target.querySelector('.canvas-region')!.getAttribute('data-tool-strip-edge')).toBe('bottom');

        toolStripPlacement.override = { edge: 'top', offset: 0.5 };
        flushSync();

        expect(target.querySelector('.toolbar')!.getAttribute('data-edge')).toBe('top');
        expect(target.querySelector('.canvas-region')!.getAttribute('data-tool-strip-edge')).toBe('top');
    });

    // The peek's headline behaviour, and the one thing that makes painting near
    // the strip intolerable if it silently regresses. jsdom reports zero rects,
    // so drive `canvasSlot` directly and assert the class, not any geometry.
    it('peek_slides_the_strip_out_on_a_nearby_move_and_never_while_a_button_is_held', () => {
        // Opt in: the strip is permanently out by default, so without this the
        // proximity listener is never installed.
        fakeConfig.set('ui.toolStrip.autoHide', true);
        const target = render();
        const strip = target.querySelector('.toolbar')!;
        canvasSlot.rect = { left: 0, top: 0, width: 1000, height: 600 };
        flushSync();

        expect(strip.classList.contains('out')).toBe(false);

        window.dispatchEvent(new PointerEvent('pointermove', { clientX: 5, clientY: 300, buttons: 0 }));
        flushSync();
        expect(strip.classList.contains('out')).toBe(true);

        window.dispatchEvent(new PointerEvent('pointermove', { clientX: 800, clientY: 300, buttons: 0 }));
        flushSync();
        expect(strip.classList.contains('out')).toBe(false);

        // A held button means a stroke, a pan or a scrub is in flight.
        window.dispatchEvent(new PointerEvent('pointermove', { clientX: 5, clientY: 300, buttons: 1 }));
        flushSync();
        expect(strip.classList.contains('out')).toBe(false);
    });

    it('color_swatches_live_in_the_tool_options_bar_not_the_tool_strip', () => {
        const target = render();

        expect(target.querySelector('.tool-options .swatches')).not.toBeNull();
        expect(target.querySelector('.toolbar .swatches')).toBeNull();
    });
});
