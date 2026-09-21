// @vitest-environment jsdom
//
// Regression: a drag whose pointer capture is lost must stop.
//
// Both of these components used to attach `pointermove` / `pointerup` to the
// captured element inside `pointerdown` and detach them only in the
// `pointerup` handler. Pointer capture is not guaranteed to end with a
// `pointerup` on that element: the browser fires `lostpointercapture` on its
// own when the pointer is removed, when the element leaves the document, or
// when another element takes capture. The pair therefore stayed attached, and
// a later plain `pointermove` kept resizing with no button held.
//
// The assertions are on the observable behaviour (does a post-capture move
// still resize?) rather than on listener bookkeeping, so they hold across the
// move to the shared `lib/pointerDrag` action, which binds once at mount
// instead of once per gesture.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const { fakeBrushGraph } = vi.hoisted(() => ({
    fakeBrushGraph: { isOpen: true, fullscreen: false },
}));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: fakeBrushGraph }));
// The panel only mounts the builder as a child; its internals are not the
// subject and pull in the whole graph editor.
vi.mock('../brush_builder/BrushBuilder.svelte', () => ({ default: function () {} }));

// `ResizeCanvasModal` reads the focused instance to seed its dimensions and to
// drive the composite preview. Neither is the subject here: the drag runs
// entirely on local rect state.
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        engine: null,
        docW: 100,
        docH: 100,
        canvasOriginX: 0,
        canvasOriginY: 0,
        requestFrame: () => {},
        onExportResult: () => {},
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => fakeApp }));

import BrushBuilderPanel from '../BrushBuilderPanel.svelte';
import ResizeCanvasModal from '../ResizeCanvasModal.svelte';
import { resizeCanvas } from '../../state/resizeCanvas.svelte';

// jsdom has neither of these; the modal observes its own size to fit the
// preview and paints the composite into a canvas, and both are irrelevant to
// the listener bookkeeping under test.
class NoopResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
}
vi.stubGlobal('ResizeObserver', NoopResizeObserver);
HTMLCanvasElement.prototype.getContext = (() => null) as unknown as HTMLCanvasElement['getContext'];
HTMLDialogElement.prototype.showModal = function () { this.open = true; };
HTMLDialogElement.prototype.close = function () { this.open = false; };

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
});

function pointer(type: string, init: Partial<PointerEvent> = {}) {
    return new PointerEvent(type, { bubbles: true, pointerId: 1, clientX: 0, clientY: 0, ...init });
}

describe('BrushBuilderPanel resize handle', () => {
    it('stops resizing when pointer capture is lost', () => {
        const target = document.createElement('div');
        document.body.append(target);
        mounted.push(mount(BrushBuilderPanel, { target }) as Record<string, unknown>);
        flushSync();

        const handle = target.querySelector('.resize-handle') as HTMLElement;
        const panel = target.querySelector('.builder-panel') as HTMLElement;
        handle.setPointerCapture = vi.fn();
        handle.releasePointerCapture = vi.fn();

        handle.dispatchEvent(pointer('pointerdown', { clientY: 400 }));
        handle.dispatchEvent(pointer('pointermove', { clientY: 300 }));
        flushSync();
        const afterDrag = panel.style.height;
        expect(afterDrag).not.toBe('');

        handle.dispatchEvent(pointer('lostpointercapture'));
        flushSync();

        // A plain move with no button held must not move the panel.
        handle.dispatchEvent(pointer('pointermove', { clientY: 100 }));
        flushSync();
        expect(panel.style.height).toBe(afterDrag);
    });
});

describe('ResizeCanvasModal drag handle', () => {
    it('stops resizing when pointer capture is lost', () => {
        resizeCanvas.open = true;
        const target = document.createElement('div');
        document.body.append(target);
        mounted.push(mount(ResizeCanvasModal, { target }) as Record<string, unknown>);
        flushSync();

        const handle = document.querySelector('.handle') as HTMLElement;
        expect(handle).toBeTruthy();
        handle.setPointerCapture = vi.fn();
        handle.releasePointerCapture = vi.fn();

        const readout = () => document.querySelector('.dims-readout')!.textContent;

        handle.dispatchEvent(pointer('pointerdown', { clientX: 0, clientY: 0 }));
        handle.dispatchEvent(pointer('pointermove', { clientX: 30, clientY: 30 }));
        flushSync();
        const afterDrag = readout();

        handle.dispatchEvent(pointer('lostpointercapture'));
        flushSync();

        handle.dispatchEvent(pointer('pointermove', { clientX: 90, clientY: 90 }));
        flushSync();
        expect(readout()).toBe(afterDrag);

        resizeCanvas.open = false;
    });
});
