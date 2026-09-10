/**
 * `use:pointerDrag`: one shared Svelte action for **same-window** capture-mode
 * drags: gutter resize and the region-width handle. It owns `setPointerCapture`,
 * the start-coordinate bookkeeping, and Escape-to-abort so no resize site
 * re-implements the pattern (DRY).
 *
 * This is deliberately NOT used for tab dragging: pointer capture traps events
 * inside one document, which would break cross-window drag. Tab drag uses
 * per-window `window`-level listeners instead (see `workspaces.svelte.ts`).
 */

export interface PointerDragParams {
    /** Pointer went down and capture began. */
    onStart?: (e: PointerEvent) => void;
    /** Movement delta from the start point, in client pixels. */
    onMove: (dx: number, dy: number, e: PointerEvent) => void;
    /** Drag finished. `aborted` is true when cancelled via Escape. */
    onEnd?: (aborted: boolean) => void;
}

export function pointerDrag(node: HTMLElement, params: PointerDragParams) {
    let current = params;
    let startX = 0;
    let startY = 0;
    let pointerId: number | null = null;

    function finish(aborted: boolean) {
        if (pointerId !== null) {
            try {
                node.releasePointerCapture(pointerId);
            } catch {
                // Capture may already be gone (pointer left the doc); ignore.
            }
            pointerId = null;
        }
        window.removeEventListener('keydown', onKey, true);
        current.onEnd?.(aborted);
    }

    function onKey(e: KeyboardEvent) {
        if (e.key === 'Escape') {
            e.preventDefault();
            finish(true);
        }
    }

    function onDown(e: PointerEvent) {
        if (e.button !== 0) return;
        e.preventDefault();
        startX = e.clientX;
        startY = e.clientY;
        pointerId = e.pointerId;
        node.setPointerCapture(e.pointerId);
        window.addEventListener('keydown', onKey, true);
        current.onStart?.(e);
    }

    function onMove(e: PointerEvent) {
        if (pointerId === null) return;
        current.onMove(e.clientX - startX, e.clientY - startY, e);
    }

    function onUp() {
        if (pointerId === null) return;
        finish(false);
    }

    node.addEventListener('pointerdown', onDown);
    node.addEventListener('pointermove', onMove);
    node.addEventListener('pointerup', onUp);

    return {
        update(next: PointerDragParams) {
            current = next;
        },
        destroy() {
            node.removeEventListener('pointerdown', onDown);
            node.removeEventListener('pointermove', onMove);
            node.removeEventListener('pointerup', onUp);
            window.removeEventListener('keydown', onKey, true);
        },
    };
}
