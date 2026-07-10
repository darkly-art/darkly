import { actions } from './registry';
import { config } from '../config/store.svelte';
import { buildChordIndex, resolveChord, type ChordEntry } from './hotkey_resolve';
import { canonicalModsFromEvent, substituteModInBinding } from './mods';
import { app } from '../state/app.svelte';
import { toolRegistry } from '../tools/registry';
import { brushGraph } from '../state/brush_graph.svelte';

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

// Drag verb vocabulary. `DRAG_VERB_BY_BUTTON` is the sole owner of the
// button→verb mapping; `DRAG_VERBS` derives the full verb set from it so the
// vocabulary lives in exactly one place — `dragChord` maps a button onto a
// verb, `dragModifierActions` enumerates them all.
const DEFAULT_DRAG_VERB = 'drag';
const DRAG_VERB_BY_BUTTON: Record<number, string> = { 1: 'middleDrag', 2: 'rightDrag' };
export const DRAG_VERBS: readonly string[] = [DEFAULT_DRAG_VERB, ...Object.values(DRAG_VERB_BY_BUTTON)];

/** Drag chord from a pointerdown event.
 *  Format: sorted modifiers joined with '+', then a button-typed drag verb.
 *  Examples: "drag", "shift+drag", "alt+rightDrag", "middleDrag". */
export function dragChord(e: PointerEvent): string {
    const mods = canonicalModsFromEvent(e);
    const verb = DRAG_VERB_BY_BUTTON[e.button] ?? DEFAULT_DRAG_VERB;
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

/** Active brush name, lowercased, for the brush dimension of a chord
 *  binding (`canvas@paint@clone:…`). Lowercasing keeps the binding string
 *  case-insensitive against the brush's display name ("Clone" → "clone").
 *  `null` when no named brush is active (a Custom edited graph). */
function activeBrushName(): string | null {
    return brushGraph.activeBrush?.toLowerCase() ?? null;
}

/** Resolve a chord at a site to its winning action, honouring specificity.
 *  The single source of truth for dispatchClick, dispatchDrag, and
 *  modifier-armed cursor engagement (`dragModifierActions`). */
function resolveChordAt(site: string, chord: string) {
    const entries = clickIndex.get(chord);
    if (!entries) return null;
    return resolveChord(entries, [{ name: site }], activeToolGroup(), activeBrushName());
}

/** The winning actions for every drag verb under a held modifier set at a
 *  site — the specificity-aware arbiter that modifier-armed cursors (the
 *  color picker, the clone set-source crosshair) gate on. Because
 *  `resolveChordAt` runs the same resolution the dispatcher uses, an
 *  engaged cursor and the eventual dispatch can never disagree about which
 *  action owns the chord. `mods === ''` yields only bare-drag bindings, so
 *  modifier-requiring actions never leak into a no-modifier hover. */
export function dragModifierActions(site: string, mods: string): Set<string> {
    const out = new Set<string>();
    for (const verb of DRAG_VERBS) {
        const chord = mods ? `${mods}+${verb}` : verb;
        const r = resolveChordAt(site, chord);
        if (r) out.add(r.entry.actionId);
    }
    return out;
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
    const resolved = resolveChordAt(site, chord);
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
    const resolved = resolveChordAt(site, dragChord(e));
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
