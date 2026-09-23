// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { pointerDrag, type PointerDragParams } from '../pointerDrag';

/** jsdom implements neither half of the pointer-capture API. */
function stubCapture(el: HTMLElement) {
    el.setPointerCapture = vi.fn();
    el.releasePointerCapture = vi.fn();
}

function pointer(type: string, init: Partial<PointerEvent> = {}) {
    return new PointerEvent(type, { bubbles: true, pointerId: 1, button: 0, clientX: 0, clientY: 0, ...init });
}

let node: HTMLElement;
let handle: ReturnType<typeof pointerDrag>;

function mount(params: Partial<PointerDragParams> = {}) {
    handle = pointerDrag(node, { onMove: () => {}, ...params });
    return handle;
}

beforeEach(() => {
    node = document.createElement('div');
    stubCapture(node);
    document.body.append(node);
});

afterEach(() => {
    handle?.destroy();
    document.body.innerHTML = '';
});

describe('pointerDrag', () => {
    it('captures on pointerdown and releases on pointerup', () => {
        const onEnd = vi.fn();
        mount({ onEnd });

        node.dispatchEvent(pointer('pointerdown'));
        expect(node.setPointerCapture).toHaveBeenCalledWith(1);

        node.dispatchEvent(pointer('pointerup'));
        expect(node.releasePointerCapture).toHaveBeenCalledWith(1);
        expect(onEnd).toHaveBeenCalledExactlyOnceWith(false);
    });

    it('reports deltas relative to the press, not to the previous move', () => {
        const onMove = vi.fn();
        mount({ onMove });

        node.dispatchEvent(pointer('pointerdown', { clientX: 100, clientY: 50 }));
        node.dispatchEvent(pointer('pointermove', { clientX: 110, clientY: 70 }));
        node.dispatchEvent(pointer('pointermove', { clientX: 130, clientY: 60 }));

        expect(onMove.mock.calls.map((c) => [c[0], c[1]])).toEqual([
            [10, 20],
            [30, 10],
        ]);
    });

    it('ends the drag when capture is lost', () => {
        const onEnd = vi.fn();
        const onMove = vi.fn();
        mount({ onEnd, onMove });

        node.dispatchEvent(pointer('pointerdown'));
        node.dispatchEvent(pointer('lostpointercapture'));
        expect(onEnd).toHaveBeenCalledExactlyOnceWith(false);

        // And no further movement is reported.
        node.dispatchEvent(pointer('pointermove', { clientX: 40 }));
        expect(onMove).not.toHaveBeenCalled();
    });

    it('ends the drag on pointercancel', () => {
        const onEnd = vi.fn();
        mount({ onEnd });
        node.dispatchEvent(pointer('pointerdown'));
        node.dispatchEvent(pointer('pointercancel'));
        expect(onEnd).toHaveBeenCalledExactlyOnceWith(false);
    });

    it('ends exactly once when several termination paths fire', () => {
        const onEnd = vi.fn();
        mount({ onEnd });
        node.dispatchEvent(pointer('pointerdown'));
        node.dispatchEvent(pointer('lostpointercapture'));
        node.dispatchEvent(pointer('pointerup'));
        expect(onEnd).toHaveBeenCalledTimes(1);
    });

    it('aborts on Escape and detaches the key listener', () => {
        const onEnd = vi.fn();
        const onMove = vi.fn();
        mount({ onEnd, onMove });

        node.dispatchEvent(pointer('pointerdown'));
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
        expect(onEnd).toHaveBeenCalledExactlyOnceWith(true);

        node.dispatchEvent(pointer('pointermove', { clientX: 40 }));
        expect(onMove).not.toHaveBeenCalled();

        // The listener is gone: a second Escape reports nothing.
        onEnd.mockClear();
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
        expect(onEnd).not.toHaveBeenCalled();
    });

    it('ignores non-primary buttons', () => {
        const onStart = vi.fn();
        mount({ onStart });
        node.dispatchEvent(pointer('pointerdown', { button: 2 }));
        expect(onStart).not.toHaveBeenCalled();
        expect(node.setPointerCapture).not.toHaveBeenCalled();
    });

    describe('onStart veto', () => {
        it('declines the gesture entirely when onStart returns false', () => {
            const onMove = vi.fn();
            mount({ onStart: () => false, onMove });

            const down = pointer('pointerdown');
            node.dispatchEvent(down);

            expect(node.setPointerCapture).not.toHaveBeenCalled();
            expect(down.defaultPrevented).toBe(false);

            node.dispatchEvent(pointer('pointermove', { clientX: 40 }));
            expect(onMove).not.toHaveBeenCalled();
        });

        it('runs before the default is suppressed, so a caller can decide', () => {
            let preventedAtStart: boolean | null = null;
            mount({ onStart: (e) => { preventedAtStart = e.defaultPrevented; } });
            node.dispatchEvent(pointer('pointerdown'));
            expect(preventedAtStart).toBe(false);
        });

        it('proceeds when onStart returns nothing', () => {
            mount({ onStart: () => {} });
            node.dispatchEvent(pointer('pointerdown'));
            expect(node.setPointerCapture).toHaveBeenCalled();
        });
    });

    describe('threshold', () => {
        it('a press that never moves never captures', () => {
            const onCapture = vi.fn();
            const onEnd = vi.fn();
            mount({ threshold: 5, onCapture, onEnd });

            node.dispatchEvent(pointer('pointerdown'));
            node.dispatchEvent(pointer('pointerup'));

            expect(node.setPointerCapture).not.toHaveBeenCalled();
            expect(onCapture).not.toHaveBeenCalled();
            expect(onEnd).not.toHaveBeenCalled();
        });

        it('a press that moves less than the threshold never captures', () => {
            const onMove = vi.fn();
            mount({ threshold: 5, onMove });
            node.dispatchEvent(pointer('pointerdown'));
            node.dispatchEvent(pointer('pointermove', { clientX: 3 }));
            expect(node.setPointerCapture).not.toHaveBeenCalled();
            expect(onMove).not.toHaveBeenCalled();
        });

        it('captures once the threshold is crossed, reporting from the press', () => {
            const onCapture = vi.fn();
            const onMove = vi.fn();
            mount({ threshold: 5, onCapture, onMove });

            node.dispatchEvent(pointer('pointerdown', { clientX: 100 }));
            node.dispatchEvent(pointer('pointermove', { clientX: 108 }));

            expect(onCapture).toHaveBeenCalledTimes(1);
            // Delta is from the press, not from the threshold crossing.
            expect(onMove).toHaveBeenCalledExactlyOnceWith(8, 0, expect.anything());
        });
    });

    describe('captureOn', () => {
        it('captures on the nominated element, not the handle', () => {
            const track = document.createElement('div');
            stubCapture(track);
            document.body.append(track);
            mount({ captureOn: () => track });

            node.dispatchEvent(pointer('pointerdown'));
            expect(track.setPointerCapture).toHaveBeenCalledWith(1);
            expect(node.setPointerCapture).not.toHaveBeenCalled();

            // And the lost-capture path is bound to that element too.
            const onEnd = vi.fn();
            handle.update({ onMove: () => {}, captureOn: () => track, onEnd });
            track.dispatchEvent(pointer('lostpointercapture'));
            expect(onEnd).toHaveBeenCalledExactlyOnceWith(false);
        });
    });

    it('destroy ends a live drag and leaves no listeners behind', () => {
        const onEnd = vi.fn();
        const onMove = vi.fn();
        mount({ onEnd, onMove });

        node.dispatchEvent(pointer('pointerdown'));
        handle.destroy();
        expect(onEnd).toHaveBeenCalledExactlyOnceWith(false);

        node.dispatchEvent(pointer('pointermove', { clientX: 40 }));
        expect(onMove).not.toHaveBeenCalled();
    });
});
