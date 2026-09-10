/**
 * Bridge between the `Document` docking panel and the persistent canvas layer.
 *
 * The WebGPU canvases (`CanvasStack`) must be mounted **exactly once**; a
 * remount destroys each canvas's WebGPU surface and its bound `DarklyHandle`.
 * But the docking system freely moves, splits, and remounts the `Document`
 * panel as the artist tiles it. To reconcile the two, the panel renders only an
 * empty *placeholder* and publishes it here; a single persistent `CanvasOverlay`
 * (mounted once at the app root) tracks the placeholder's rect and positions
 * itself over it. The canvas thus *follows* the panel around the tree without
 * ever remounting.
 *
 * `null` when no `Document` panel is currently mounted (e.g. it's the inactive
 * tab of a group), so the overlay hides in that case.
 */
class CanvasSlot {
    current = $state<HTMLElement | null>(null);

    set(el: HTMLElement) {
        this.current = el;
    }

    /** Clear only if `el` is still the published slot: avoids a late unmount
     *  wiping a newer panel's registration (mount order isn't guaranteed). */
    clear(el: HTMLElement) {
        if (this.current === el) this.current = null;
    }
}

export const canvasSlot = new CanvasSlot();
