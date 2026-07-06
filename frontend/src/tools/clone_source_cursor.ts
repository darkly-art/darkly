import { app } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { brushGraph } from '../state/brush_graph.svelte';
import { effectiveMouseClicks } from '../actions/triggers';
import { parseBinding } from '../actions/hotkey_resolve';
import { canonicalModsFromEvent, substituteMod } from '../actions/mods';
import { config } from '../config/store.svelte';
import { modPrefixOfChord } from './colorpicker_cursor';

// Clone set-source cursor + arming. Mirrors `colorpicker_cursor.ts`: the
// arming modifier is derived at runtime from whatever `setCloneSource` is
// bound to in the active config layer (so preset swaps + user overrides
// flow through), and a crosshair cursor prompts / confirms the gesture.
//
// Armed conditions (crosshair shown) when a paint tool is active and the
// active brush needs a source (`activeBrushNeedsSource`, cached async):
//   1. No source anchor is set yet — a persistent prompt so a clone brush
//      never silently no-op paints (the Rust no-op gate skips the stroke).
//   2. A source is set AND the user holds the set-source modifier — armed
//      to re-set it.
//
// The gesture itself is dispatched by the brush-scoped chord binding
// through `dispatchDrag`; this module only owns the cursor + the cached
// "needs source" flag.

// ---------------------------------------------------------------------------
// Engine-backed anchor + needs-source cache
// ---------------------------------------------------------------------------

let hasSource = false;
let needsSource = false;
let lastBrush: string | null | undefined = undefined;
let needsQueryInFlight = false;

/** Record the clone source anchor in the engine (plane / canvas pixels)
 *  and mark that a source now exists so the cursor stops prompting. */
export function setCloneSourceAnchor(cx: number, cy: number): void {
    app.engine?.api.setCloneSource({ x: cx, y: cy });
    hasSource = true;
    refreshCursor();
    app.requestFrame();
}

/** Re-query whether the active brush needs a source whenever the active
 *  brush changes. Async (engine round-trip); the cached result drives
 *  arming. Switching brushes resets the has-source prompt but not the
 *  engine anchor — the anchor persists as session state, so a clone brush
 *  reselected mid-session keeps its source. */
function syncNeedsSource(): void {
    const brush = brushGraph.activeBrush ?? null;
    if (brush === lastBrush) return;
    lastBrush = brush;
    const engine = app.engine;
    if (!engine || needsQueryInFlight) return;
    needsQueryInFlight = true;
    engine.api
        .activeBrushNeedsSource()
        .then((v: boolean) => {
            needsSource = v;
            refreshCursor();
            app.requestFrame();
        })
        .finally(() => {
            needsQueryInFlight = false;
        });
}

// ---------------------------------------------------------------------------
// Modifier tracking + arming
// ---------------------------------------------------------------------------

let currentMods = '';
let engagementMods: Set<string> = new Set();
let lastCursorKey: string | null = null;

/** Modifier prefixes that arm the set-source cursor, derived from the
 *  effective `setCloneSource` bindings (brush-scoped or group-scoped for
 *  clone). Preset swaps + user overrides flow through automatically. */
function cloneEngagementMods(): Set<string> {
    const out = new Set<string>();
    for (const raw of effectiveMouseClicks('setCloneSource')) {
        const { site, scope, brush, chord } = parseBinding(raw);
        if (site !== null && site !== 'canvas') continue;
        if (scope !== null && scope !== 'paint') continue;
        if (brush !== null && brush !== 'clone') continue;
        const prefix = modPrefixOfChord(substituteMod(chord));
        // A bare-drag binding (no modifier) would arm on hover and fight
        // every stroke; skip it (the no-source prompt below still shows).
        if (prefix === null || prefix === '') continue;
        out.add(prefix);
    }
    return out;
}

function isPaintToolActive(): boolean {
    return toolRegistry.get(app.activeToolId)?.group === 'paint';
}

/** The crosshair is shown when the active paint brush needs a source and
 *  either no source is set (a prompt) or the arming modifier is held. */
function isArmed(): boolean {
    if (!needsSource || !isPaintToolActive()) return false;
    if (!hasSource) return true;
    return engagementMods.has(currentMods);
}

function refreshCursor(): void {
    const armed = isArmed();
    const key = armed ? 'armed' : 'idle';
    if (key === lastCursorKey) return;
    lastCursorKey = key;
    if (armed) {
        app.toolCursor = 'crosshair';
    } else if (app.toolCursor === 'crosshair') {
        // Only release the cursor if we own it — don't stomp another
        // module's override (e.g. the color picker's dropper).
        app.toolCursor = null;
    }
}

/** Per-frame tick — refreshes the needs-source cache on brush change and
 *  re-evaluates the cursor. Cheap when nothing changed (memo guards). */
export function tickCloneSourceCursor(): void {
    syncNeedsSource();
    refreshCursor();
}

function modsFromEvent(e: {
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
    metaKey: boolean;
}): string {
    return canonicalModsFromEvent(e).join('+');
}

let wired = false;

/** Wire global modifier tracking for the set-source cursor. Idempotent.
 *  The arming modifier set is sourced from the `setCloneSource` binding so
 *  preset swaps / user overrides flow through. */
export function setupCloneSourceModifierTracking(): void {
    if (wired) return;
    wired = true;

    engagementMods = cloneEngagementMods();
    config.onChange(() => {
        engagementMods = cloneEngagementMods();
        refreshCursor();
    });

    const onKey = (e: KeyboardEvent) => {
        const next = modsFromEvent(e);
        if (next === currentMods) return;
        currentMods = next;
        refreshCursor();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKey);
    window.addEventListener('blur', () => {
        if (currentMods !== '') {
            currentMods = '';
            refreshCursor();
        }
    });
    window.addEventListener('pointermove', (e) => {
        const next = modsFromEvent(e);
        if (next !== currentMods) {
            currentMods = next;
            refreshCursor();
        }
    });
}
