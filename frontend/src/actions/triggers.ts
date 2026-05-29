import { actions } from './registry';
import { config } from '../config/store.svelte';
import { buildChordIndex, resolveChord, type ChordEntry } from './hotkey_resolve';
import { canonicalModsFromEvent, substituteModInBinding } from './mods';
import { app } from '../state/app.svelte';
import { toolRegistry } from '../tools/registry';

/** Derive a canonical chord from a MouseEvent's modifier state.
 *  Format: sorted modifiers joined with '+', then the interaction type.
 *  Examples: "click", "alt+click", "ctrl+shift+doubleClick".
 *  Primitive modifier vocabulary — `$mod` is resolved at chord-index
 *  build time (see `rebuildClickIndex`), not here. */
export function chordName(e: MouseEvent): string {
    const mods = canonicalModsFromEvent(e);

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
 *  Examples: "drag", "shift+drag", "alt+rightDrag", "middleDrag". */
export function dragChord(e: PointerEvent): string {
    const mods = canonicalModsFromEvent(e);

    const verb =
        e.button === 1 ? 'middleDrag'
        : e.button === 2 ? 'rightDrag'
        : 'drag';

    return mods.length > 0 ? `${mods.join('+')}+${verb}` : verb;
}

/**
 * Resolve an action's effective mouse trigger list. The binding lives in
 * `mouseclicks.<id>` under the three-layer config — defaults.yaml +
 * overlay + user override. Multi-binding entries (e.g. `isolateLayer`
 * firing from both `layerThumb:alt+click` and `maskThumb:alt+click`) are
 * joined with `|` in the YAML parser; we split them back here.
 *
 * Format: each entry is `"<site>:<chord>"`. Empty string means
 * "no mouse trigger" — used by overlays that explicitly disable a binding.
 */
export function effectiveMouseClicks(actionId: string): string[] {
    const v = config.get(`mouseclicks.${actionId}`);
    if (typeof v !== 'string') return [];
    if (!v) return [];
    return v.split('|').filter(Boolean);
}

/**
 * Single-string view of an action's effective mouse trigger, for the
 * Settings UI's input row. Returns the first binding from
 * `effectiveMouseClicks` (or "" if none). The UI's "reset to default"
 * button still drops back to the full default list at dispatch time.
 */
export function effectiveMouseClick(actionId: string): string {
    return effectiveMouseClicks(actionId)[0] ?? '';
}

/**
 * `chord → ordered ChordEntry[]` lookup table built from the action registry +
 * any `mouseclicks.<id>` overrides in config. Rebuilt via `rebuildClickIndex`
 * at startup and on every config change.
 *
 * The index covers both click chords (`click`, `alt+doubleClick`, …) and
 * drag chords (`drag`, `shift+drag`, `alt+rightDrag`, …). The chord vocabularies
 * are non-overlapping so a single map is sufficient.
 *
 * Resolution: at dispatch time, `resolveChord` filters entries by the click
 * site (passed by the caller) and the active tool's group, picking the most
 * specific match. See `hotkey_resolve.ts` for the binding-string grammar.
 */
let clickIndex: Map<string, ChordEntry[]> = new Map();

export function rebuildClickIndex() {
    clickIndex = buildChordIndex(
        actions.all().map(a => ({
            actionId: a.id,
            // Resolve `$mod` to the platform's primitive (`ctrl`/`meta`) once,
            // here, so the runtime matcher in `dispatchClick`/`dispatchDrag`
            // compares literal-vs-literal. Tinykeys does the same for keyboard
            // bindings internally; mouse chords need their own pass because
            // they don't go through tinykeys.
            bindings: effectiveMouseClicks(a.id).map(substituteModInBinding),
        })),
    );
}

/** Active tool's `group` (e.g. `"paint"`, `"select"`), or `null` if no tool. */
function activeToolGroup(): string | null {
    return toolRegistry.get(app.activeToolId)?.group ?? null;
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
    const entries = clickIndex.get(chord);
    if (!entries) return false;
    const resolved = resolveChord(entries, [{ name: site }], activeToolGroup());
    if (!resolved) return false;
    actions.dispatch(resolved.entry.actionId, ctx);
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
    const entries = clickIndex.get(chord);
    if (!entries) return false;
    const resolved = resolveChord(entries, [{ name: site }], activeToolGroup());
    if (!resolved) return false;
    const actionId = resolved.entry.actionId;

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
