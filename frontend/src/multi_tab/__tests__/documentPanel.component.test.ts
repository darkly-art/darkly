// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The tool strip and the tool-options bar both resolve tooltips through the
// config store, which is WASM-backed in production.
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    tooltipForAction: (label: string) => label,
}));

// `ToolOptionsBar` imports the brush graph, which transitively pulls in the
// brush library and recents modules; those touch persistent storage at import
// time. Only `fullscreen` is read here.
vi.mock('../../state/brush_graph.svelte', () => ({
    brushGraph: { fullscreen: false },
}));

import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import { menuBar } from '../../state/menuBar.svelte';
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
// `CanvasOverlay` is not mounted); the comment in `ToolStrip.svelte` is the
// defence for that one.
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

    it('color_swatches_live_in_the_tool_options_bar_not_the_tool_strip', () => {
        const target = render();

        expect(target.querySelector('.tool-options .swatches')).not.toBeNull();
        expect(target.querySelector('.toolbar .swatches')).toBeNull();
    });
});
