import { actions } from './registry';
import { config } from '../config/store.svelte';

/** Derive a canonical chord from a MouseEvent's modifier state.
 *  Format: sorted modifiers joined with '+', then the interaction type.
 *  Examples: "click", "alt+click", "ctrl+shift+doubleClick" */
export function chordName(e: MouseEvent): string {
    const mods: string[] = [];
    if (e.ctrlKey || e.metaKey) mods.push('ctrl');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');

    let interaction: string;
    if (e.button === 1) {
        interaction = 'middleClick';
    } else if (e.detail === 2) {
        interaction = 'doubleClick';
    } else {
        interaction = 'click';
    }

    return mods.length > 0 ? `${mods.join('+')}+${interaction}` : interaction;
}

/** Drag chord from a pointerdown event.
 *  Format: sorted modifiers joined with '+', then a button-typed drag verb.
 *  Examples: "drag", "shift+drag", "alt+rightDrag", "middleDrag" */
export function dragChord(e: PointerEvent): string {
    const mods: string[] = [];
    if (e.ctrlKey || e.metaKey) mods.push('ctrl');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');

    const verb =
        e.button === 1 ? 'middleDrag'
        : e.button === 2 ? 'rightDrag'
        : 'drag';

    return mods.length > 0 ? `${mods.join('+')}+${verb}` : verb;
}

/**
 * Resolve an action's effective mouse trigger:
 *   user override (`mouseclicks.<id>`) ?? action.defaultMouseClick ?? unbound.
 *
 * Format: `"<site>:<chord>"` (e.g. `"layerEye:alt+click"`). Empty string
 * suppresses any default — use it in a preset to remove a click trigger.
 */
export function effectiveMouseClick(actionId: string): string {
    const override = config.get(`mouseclicks.${actionId}`);
    if (typeof override === 'string') return override;
    const action = actions.get(actionId);
    return action?.defaultMouseClick ?? '';
}

/**
 * `(site, chord) -> actionId` lookup table built from the action registry +
 * any `mouseclicks.<id>` overrides in config. Rebuilt via `rebuildClickIndex`
 * at startup and on every config change.
 *
 * The index covers both click chords (`click`, `alt+doubleClick`, …) and
 * drag chords (`drag`, `shift+drag`, `alt+rightDrag`, …). The chord vocabularies
 * are non-overlapping so a single map is sufficient.
 */
let clickIndex: Map<string, string> = new Map();

export function rebuildClickIndex() {
    const next = new Map<string, string>();
    for (const action of actions.all()) {
        const trigger = effectiveMouseClick(action.id);
        if (!trigger) continue;
        // Last-wins on conflicts; the Settings UI's hotkey tab will surface
        // these as warnings via the same conflict-detection pattern keyboard
        // hotkeys use.
        next.set(trigger, action.id);
    }
    clickIndex = next;
}

/** Look up a click on `(site, e)` and dispatch the bound action if any.
 *  Returns true if a binding existed and was dispatched. */
export function dispatchClick(
    site: string,
    e: MouseEvent,
    ctx: Record<string, any>,
): boolean {
    const chord = chordName(e);
    if (chord === 'click') return false; // plain click = component default
    const actionId = clickIndex.get(`${site}:${chord}`);
    if (!actionId) return false;
    actions.dispatch(actionId, ctx);
    return true;
}

/**
 * Look up a drag on `(site, e)` and, if a binding exists, take over the
 * pointer's down/move/up lifecycle and route it to the action.
 *
 * On match: captures the pointer, dispatches the action's `handler` (the
 * "down" hook), wires window-level `pointermove → action.onMove(ctx, dx, dy)`
 * and `pointerup → action.deactivate(ctx)`, and returns `true` so callers can
 * short-circuit any tool that would otherwise see the pointer event.
 *
 * `dx`/`dy` are deltas in client pixels from the original pointerdown.
 */
export function dispatchDrag(
    site: string,
    e: PointerEvent,
    ctx: Record<string, any>,
): boolean {
    const chord = dragChord(e);
    const actionId = clickIndex.get(`${site}:${chord}`);
    if (!actionId) return false;

    const target = e.currentTarget as Element | null;
    target?.setPointerCapture?.(e.pointerId);

    // Thread the original pointerdown event through ctx so handlers can
    // freeze pose (pressure / tilt / twist) at the start of the drag.
    const dragCtx = { ...ctx, event: e };
    actions.dispatch(actionId, dragCtx);

    const startX = e.clientX;
    const startY = e.clientY;

    const onMove = (ev: PointerEvent) => {
        const action = actions.get(actionId);
        action?.onMove?.(dragCtx, ev, ev.clientX - startX, ev.clientY - startY);
    };
    const onUp = (ev: PointerEvent) => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
        window.removeEventListener('pointercancel', onUp);
        target?.releasePointerCapture?.(ev.pointerId);
        actions.release(actionId, dragCtx);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);

    return true;
}
