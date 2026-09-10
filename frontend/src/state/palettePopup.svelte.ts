/**
 * The palette popup's store: the gesture machine made reactive, plus the
 * effect runner that turns a commit into a `select()` call.
 *
 * Input arrives exclusively through the drag-chord dispatcher's
 * `handler`/`onMove`/`deactivate` (see `actions/palette_popup.ts`); the
 * overlay component is display-only and never feeds events back.
 */
import { SvelteMap } from 'svelte/reactivity';
import { app } from './app.svelte';
import {
    CLOSED,
    reduce,
    type MachineEvent,
    type MachineState,
} from '../ui/palette_popup/machine';
import { hitKey } from '../ui/palette_popup/wheel_geometry';
import { nodeAt, paletteSections, type WheelTree } from '../ui/palette_popup/model';

/** Pointermove arrives at display rate; skip the reactive write when the
 *  sample changed nothing the wheel shows. The cursor marker tracks every
 *  sample, so this only ever elides exact-duplicate positions. */
function equivalent(a: MachineState, b: MachineState): boolean {
    if (a === b) return true;
    if (a.kind !== 'engaged' || b.kind !== 'engaged') return false;
    return (
        a.pointerId === b.pointerId &&
        a.center.x === b.center.x &&
        a.center.y === b.center.y &&
        a.cursor.x === b.cursor.x &&
        a.cursor.y === b.cursor.y &&
        a.path.length === b.path.length &&
        a.path.every((v, i) => b.path[i] === v) &&
        hitKey(a.highlight) === hitKey(b.highlight)
    );
}

class PalettePopupStore {
    state = $state<MachineState>(CLOSED);
    /** Snapshotted at open; stable for the gesture's lifetime. */
    tree = $state<WheelTree>({ sections: [] });
    /** Each label's rendered length in px, by label string.
     *
     *  Keyed by the string and not by the sector, because a width is a property
     *  of a name in a font and not of a place on the wheel: two sectors showing
     *  the same name measure once, and a name's width does not go stale when the
     *  wheel re-lays under it.
     *
     *  It lives here rather than in the component because both the component's
     *  paint and the machine's hit-test have to agree about it. That is the
     *  "paint and hit can never disagree" invariant, and a measurement only one
     *  of them could see would break it. Cleared at open, beside the snapshot
     *  it belongs with. */
    labelWidths = new SvelteMap<string, number>();

    get isOpen(): boolean {
        return this.state.kind === 'engaged';
    }

    /** The dispatcher's down hook. No-ops while a stroke is in flight (a
     *  second pointerdown mid-stroke must not summon the wheel over it). */
    open(e: PointerEvent): void {
        if (this.state.kind !== 'closed') return;
        if (app.pointerActive) return;
        this.labelWidths.clear();
        this.tree = paletteSections.snapshot();
        this.#apply({ kind: 'down', pointerId: e.pointerId, x: e.clientX, y: e.clientY });
    }

    move(e: PointerEvent): void {
        this.#apply({ kind: 'move', pointerId: e.pointerId, x: e.clientX, y: e.clientY });
    }

    /** The dispatcher's release hook. It fires for both `pointerup` and
     *  `pointercancel`; a cancel must not commit, so the terminating event's
     *  type routes it. Without an event (defensive), release as the latched
     *  pointer. */
    release(e?: PointerEvent): void {
        if (e?.type === 'pointercancel') {
            this.cancel();
            return;
        }
        const pointerId =
            e?.pointerId ?? (this.state.kind === 'engaged' ? this.state.pointerId : -1);
        this.#apply({ kind: 'up', pointerId });
    }

    cancel(): void {
        if (this.state.kind === 'closed') return;
        this.#apply({ kind: 'cancel' });
    }

    #apply(event: MachineEvent): void {
        const { state, effect } = reduce(this.state, event, this.tree, this.labelWidths);
        if (!equivalent(this.state, state)) this.state = state;
        if (effect?.kind === 'commit') {
            const node = nodeAt(this.tree, effect.path);
            if (node?.kind === 'leaf') node.select();
        }
    }
}

export const palettePopup = new PalettePopupStore();
