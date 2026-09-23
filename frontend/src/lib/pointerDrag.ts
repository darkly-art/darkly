/**
 * `use:pointerDrag`: the one pointer-capture drag in the frontend.
 *
 * A capture drag is the same five steps everywhere: take the pointer on
 * `pointerdown`, report movement relative to where it went down, and end
 * exactly once however the gesture finishes. The "however" is the part every
 * hand-rolled copy got wrong: a capture does not always end with a `pointerup`
 * on the capturing element. The browser ends it on its own when the pointer is
 * removed, when the element leaves the document, or when something else takes
 * capture, and it announces that with `lostpointercapture`. A drag that only
 * listens for `pointerup` leaves its state set and its listeners attached.
 *
 * Five termination paths, all routed through one idempotent `finish`:
 * `pointerup`, `lostpointercapture`, `pointercancel`, Escape (which reports
 * `aborted`), and `destroy`.
 *
 * Deliberately NOT used for tab dragging: pointer capture traps events inside
 * one document, which would break cross-window drag. Tab drag uses per-window
 * `window`-level listeners instead (see `multi_tab/TabStrip.svelte` and
 * `ui/workspace/workspaces.svelte.ts`). `canvas/CanvasView.svelte` is the other
 * exception: it captures for stroke input, where the tool system rather than a
 * delta callback owns the gesture.
 *
 * Value semantics (preview while moving, commit once at the end) are a separate
 * concern and live in `lib/scrubDrag.ts`, which is DOM-free so it can be tested
 * without a `window`. The two compose: start a `beginScrubDrag` inside
 * `onStart`, route `onMove` into `drag.move` and `onEnd` into `drag.end`.
 */

/** Anything with the pointer-capture API and pointer events in its event map:
 *  an `HTMLElement`, or the `<svg>` and `<rect>` handles the curve editor and
 *  the resize preview drag. `Element` is too wide, since pointer events are
 *  declared on these two subtypes rather than on the base. */
export type DragTarget = HTMLElement | SVGElement;

export interface PointerDragParams {
    /**
     * Pointer went down, before anything has been captured or defaulted.
     *
     * Return `false` to decline the gesture: no capture is taken, no listeners
     * are bound, and the event is left entirely alone. This is the hook for
     * "only some presses start a drag" (a hit test, a modifier check, a button
     * filter beyond the left-button default) and it runs early enough that a
     * caller can also decide for itself whether to `preventDefault`.
     */
    onStart?: (e: PointerEvent) => boolean | void;
    /**
     * Capture has been taken and the drag is live. With a `threshold` this
     * fires when the threshold is crossed, not on the press, which is the seam
     * between "selected it" and "started moving it".
     */
    onCapture?: (e: PointerEvent) => void;
    /** Movement delta from the start point, in client pixels. The event is
     *  passed through for callers that want absolute coordinates instead. */
    onMove: (dx: number, dy: number, e: PointerEvent) => void;
    /** Drag finished. `aborted` is true when cancelled via Escape. Fires at
     *  most once per gesture. */
    onEnd?: (aborted: boolean) => void;
    /**
     * Defer capture until the pointer has moved this many client pixels.
     * `0` (the default) captures on `pointerdown`. A press that never crosses
     * the threshold never captures and never calls `onCapture` or `onEnd`, so a
     * plain click stays a plain click.
     */
    threshold?: number;
    /**
     * Element to capture on, when the handle and the frame of reference differ:
     * a slider's small grab handle capturing on its full-width track, so moves
     * keep resolving past the end of the handle. Defaults to the node itself.
     */
    captureOn?: () => DragTarget;
    /** Suppress the browser default on `pointerdown` (text selection, pen
     *  fling-scroll). Default true. */
    preventDefault?: boolean;
}

/**
 * Register a pointer handler.
 *
 * `addEventListener` on a `HTMLElement | SVGElement` union resolves to the
 * untyped base overload rather than the pointer-typed one, because TypeScript
 * cannot pick an overload across a union receiver. Both members declare the
 * same pointer event map, so the cast is sound, and doing it here keeps it to
 * one place instead of four call sites.
 */
function on(el: DragTarget, type: string, fn: (e: PointerEvent) => void) {
    el.addEventListener(type, fn as EventListener);
}

function off(el: DragTarget, type: string, fn: (e: PointerEvent) => void) {
    el.removeEventListener(type, fn as EventListener);
}

export function pointerDrag(node: DragTarget, params: PointerDragParams) {
    let current = params;
    let startX = 0;
    let startY = 0;
    let pointerId: number | null = null;
    /** Set between an accepted pointerdown and capture actually being taken;
     *  only meaningful when a threshold defers that. */
    let pending = false;

    function target(): DragTarget {
        return current.captureOn?.() ?? node;
    }

    function capture(e: PointerEvent) {
        pending = false;
        pointerId = e.pointerId;
        const el = target();
        try {
            el.setPointerCapture(e.pointerId);
        } catch {
            // Capture can be refused (the pointer is already gone); the drag
            // still runs off the listeners below.
        }
        el.addEventListener('lostpointercapture', onLost);
        window.addEventListener('keydown', onKey, true);
        current.onCapture?.(e);
    }

    function finish(aborted: boolean) {
        pending = false;
        if (pointerId === null) return;
        const el = target();
        try {
            el.releasePointerCapture(pointerId);
        } catch {
            // Already gone (the pointer left the document); ignore.
        }
        pointerId = null;
        el.removeEventListener('lostpointercapture', onLost);
        window.removeEventListener('keydown', onKey, true);
        current.onEnd?.(aborted);
    }

    function onLost() {
        finish(false);
    }

    function onKey(e: KeyboardEvent) {
        if (e.key === 'Escape') {
            e.preventDefault();
            finish(true);
        }
    }

    function onDown(e: PointerEvent) {
        if (e.button !== 0) return;
        if (current.onStart?.(e) === false) return;
        if (current.preventDefault !== false) e.preventDefault();
        startX = e.clientX;
        startY = e.clientY;
        if ((current.threshold ?? 0) > 0) {
            pending = true;
            return;
        }
        capture(e);
    }

    function onMove(e: PointerEvent) {
        if (pending) {
            const t = current.threshold ?? 0;
            if (Math.hypot(e.clientX - startX, e.clientY - startY) < t) return;
            capture(e);
        }
        if (pointerId === null) return;
        current.onMove(e.clientX - startX, e.clientY - startY, e);
    }

    function onUp() {
        pending = false;
        if (pointerId === null) return;
        finish(false);
    }

    on(node, 'pointerdown', onDown);
    on(node, 'pointermove', onMove);
    on(node, 'pointerup', onUp);
    on(node, 'pointercancel', onUp);

    return {
        update(next: PointerDragParams) {
            current = next;
        },
        destroy() {
            finish(false);
            off(node, 'pointerdown', onDown);
            off(node, 'pointermove', onMove);
            off(node, 'pointerup', onUp);
            off(node, 'pointercancel', onUp);
            window.removeEventListener('keydown', onKey, true);
        },
    };
}
