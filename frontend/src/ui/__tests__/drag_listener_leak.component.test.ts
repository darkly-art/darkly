// @vitest-environment jsdom
//
// Regression: a drag whose pointer capture is lost must detach its listeners.
//
// Both of these components attach `pointermove` / `pointerup` to the captured
// element inside `pointerdown` and detach them in the `pointerup` handler.
// Pointer capture is not guaranteed to end with a `pointerup` on that element:
// the browser fires `lostpointercapture` on its own when the pointer is
// removed, when the element leaves the document, or when another element takes
// capture. Without a handler for it the pair stays attached, so a later plain
// `pointermove` over the control keeps resizing with no button held, and every
// subsequent drag adds another pair.
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
    it('detaches its drag listeners when pointer capture is lost', () => {
        const target = document.createElement('div');
        document.body.append(target);
        mounted.push(mount(BrushBuilderPanel, { target }) as Record<string, unknown>);
        flushSync();

        const handle = target.querySelector('.resize-handle') as HTMLElement;
        handle.setPointerCapture = vi.fn();
        handle.releasePointerCapture = vi.fn();

        // Count what the drag attaches, so the assertion is about the listeners
        // themselves rather than about a height value that may legitimately
        // settle anywhere.
        const added: string[] = [];
        const removed: string[] = [];
        const realAdd = handle.addEventListener.bind(handle);
        const realRemove = handle.removeEventListener.bind(handle);
        handle.addEventListener = (type: string, ...rest: unknown[]) => {
            added.push(type);
            return realAdd(type, ...(rest as [EventListenerOrEventListenerObject]));
        };
        handle.removeEventListener = (type: string, ...rest: unknown[]) => {
            removed.push(type);
            return realRemove(type, ...(rest as [EventListenerOrEventListenerObject]));
        };

        handle.dispatchEvent(pointer('pointerdown'));
        flushSync();
        expect(added).toContain('pointermove');

        handle.dispatchEvent(pointer('lostpointercapture'));
        flushSync();

        expect(removed).toContain('pointermove');
        expect(removed).toContain('pointerup');
    });
});

describe('ResizeCanvasModal drag handle', () => {
    it('detaches its drag listeners when pointer capture is lost', () => {
        resizeCanvas.open = true;
        const target = document.createElement('div');
        document.body.append(target);
        mounted.push(mount(ResizeCanvasModal, { target }) as Record<string, unknown>);
        flushSync();

        const handle = document.querySelector('.handle') as HTMLElement;
        expect(handle).toBeTruthy();
        handle.setPointerCapture = vi.fn();
        handle.releasePointerCapture = vi.fn();

        const added: string[] = [];
        const removed: string[] = [];
        const realAdd = handle.addEventListener.bind(handle);
        const realRemove = handle.removeEventListener.bind(handle);
        handle.addEventListener = (type: string, ...rest: unknown[]) => {
            added.push(type);
            return realAdd(type, ...(rest as [EventListenerOrEventListenerObject]));
        };
        handle.removeEventListener = (type: string, ...rest: unknown[]) => {
            removed.push(type);
            return realRemove(type, ...(rest as [EventListenerOrEventListenerObject]));
        };

        handle.dispatchEvent(pointer('pointerdown'));
        flushSync();
        expect(added).toContain('pointermove');

        handle.dispatchEvent(pointer('lostpointercapture'));
        flushSync();

        expect(removed).toContain('pointermove');
        expect(removed).toContain('pointerup');

        resizeCanvas.open = false;
    });
});
