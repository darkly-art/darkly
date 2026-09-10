/**
 * Per-brush color memory: the paint stays on the brush.
 *
 * A real brush carries whatever paint it was last dipped in, so picking it up
 * again brings that color back. This module remembers, per brush id, the
 * foreground/background pair the brush was last used with. `BrushGraphState`
 * records the outgoing brush's pair on every switch (locked or not, so
 * turning the lock on later already has history) and restores the incoming
 * brush's pair when `colors.lockToBrush` is set.
 *
 * Keyed by brush id rather than name so a rename keeps the memory. Module
 * level rather than per `DarklyInstance`: brushes are global, and the
 * metaphor is paint on the brush, not on the canvas.
 *
 * This is session state under the Document Authority principle: it must not
 * ride a `.darkly` file (it is not part of the document) and it cannot be
 * rebuilt from one (it is not compositor state), so it lives for the tab's
 * lifetime only. The lock itself is a preference and lives in config; this
 * module never consults it, so the policy stays at the one call site and the
 * memory stays a plain map.
 */
import type { Color } from './app.svelte';

export interface ColorPair {
    foreground: Color;
    background: Color;
}

class BrushColorMemory {
    #pairs = new Map<string, ColorPair>();

    /** Remember `pair` as the paint on brush `id`. Stores copies, so later
     *  edits to the live swatches do not rewrite the memory. A null id (a
     *  graph that came from no library brush) has nothing to remember under. */
    record(id: string | null, pair: ColorPair) {
        if (id === null) return;
        this.#pairs.set(id, {
            foreground: { ...pair.foreground },
            background: { ...pair.background },
        });
    }

    /** Put brush `id`'s remembered paint back onto `target`. Returns whether
     *  anything was restored: a brush that was never recorded leaves the pair
     *  untouched, so it inherits the current colors and records them on the
     *  next switch away. */
    restore(id: string | null, target: ColorPair): boolean {
        if (id === null) return false;
        const pair = this.#pairs.get(id);
        if (!pair) return false;
        target.foreground = { ...pair.foreground };
        target.background = { ...pair.background };
        return true;
    }

    /** Forget every brush. Test seam. */
    clear() {
        this.#pairs.clear();
    }
}

export const brushColors = new BrushColorMemory();
