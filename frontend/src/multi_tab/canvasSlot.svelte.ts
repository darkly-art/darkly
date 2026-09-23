/**
 * Bridge between the `Document` docking panel and the persistent canvas layer.
 *
 * The WebGPU canvases (`CanvasStack`) must be mounted **exactly once**; a
 * remount destroys each canvas's WebGPU surface and its bound `DarklyHandle`.
 * But the docking system freely moves, splits, and remounts the `Document`
 * panel as the artist tiles it. To reconcile the two, the panel renders a
 * *placeholder* region in place of the canvas and publishes it here (the
 * floating tool strip rides inside that region, but nothing else does); a
 * single persistent `CanvasOverlay` (mounted once at the app root) tracks the
 * placeholder's rect and positions itself over it. The canvas thus *follows*
 * the panel around the tree without ever remounting.
 *
 * The slot owns the rect rather than the overlay, because the tool strip needs
 * the same measurement to place itself along an edge of the region, and one
 * `getBoundingClientRect` reader is the rule (`docs/coordinate-systems.md`).
 * This is screen/CSS space: no device-pixel-ratio, no plane coordinates.
 *
 * `null` when no `Document` panel is currently mounted (e.g. it's the inactive
 * tab of a group), so the overlay hides in that case.
 */
import type { Rect } from '../lib/edges';

class CanvasSlot {
    current = $state<HTMLElement | null>(null);
    rect = $state<Rect | null>(null);

    /** Live only while a slot is published; `clear()` is the sole teardown. */
    #observer: ResizeObserver | null = null;
    #onWindowResize = () => this.reposition();

    set(el: HTMLElement) {
        // Mount order isn't guaranteed, so a new panel can register before the
        // old one unmounts. Tear the previous watch down rather than leaking it.
        this.#unwatch();
        this.current = el;
        this.reposition();

        // Covers gutter drags, which resize the slot without remounting it.
        if (typeof ResizeObserver !== 'undefined') {
            this.#observer = new ResizeObserver(() => this.reposition());
            this.#observer.observe(el);
        }
        window.addEventListener('resize', this.#onWindowResize);
    }

    /** Clear only if `el` is still the published slot: avoids a late unmount
     *  wiping a newer panel's registration (mount order isn't guaranteed). */
    clear(el: HTMLElement) {
        if (this.current !== el) return;
        this.#unwatch();
        this.current = null;
        this.rect = null;
    }

    /** Re-measure the published slot. Called on resize, and by `CanvasOverlay`
     *  after a tiling mutation settles (a gutter drag can move the slot without
     *  resizing it, which no ResizeObserver reports). */
    reposition() {
        const el = this.current;
        if (!el) {
            this.rect = null;
            return;
        }
        const r = el.getBoundingClientRect();
        this.rect = { left: r.left, top: r.top, width: r.width, height: r.height };
    }

    #unwatch() {
        this.#observer?.disconnect();
        this.#observer = null;
        window.removeEventListener('resize', this.#onWindowResize);
    }
}

export const canvasSlot = new CanvasSlot();
