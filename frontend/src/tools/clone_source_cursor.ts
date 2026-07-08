import { app } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { brushGraph } from '../state/brush_graph.svelte';
import { dragModifierActions } from '../actions/triggers';
import { heldMods, onHeldModsChange } from '../actions/held_mods';
import { config } from '../config/store.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import { canvasToScreen } from '../canvas/coordinates';

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

interface Pt {
    x: number;
    y: number;
}

let hasSource = false;
let needsSource = false;
/** Aligned (false) vs anchored (true), cached from the engine alongside
 *  `needsSource`. Drives the on-canvas marker's stroke tracking. */
let anchoredMode = false;
/** The set source anchor in canvas / plane pixels — a local mirror of the
 *  engine's `clone_source_anchor` (the engine exposes no getter). */
let sourceAnchor: Pt | null = null;
/** Destination anchor captured at `pointerdown` (canvas pixels), mirroring
 *  the engine's per-stroke first-dab `dest_anchor`. Non-null only while a
 *  clone stroke is in flight. */
let destAnchor: Pt | null = null;
/** Latest canvas cursor position — the stroke's live dab centre while
 *  painting, or the hover position otherwise. Anchors both the tracked
 *  source marker (aligned mode) and the no-source hint. */
let cursorPos: Pt | null = null;
let lastBrush: string | null | undefined = undefined;
let needsQueryInFlight = false;

/** Where the source marker sits for a given cursor position. Anchored mode
 *  pins it at the set source; aligned mode slides it by the same offset the
 *  cursor has travelled from the stroke's dest anchor — matching
 *  `clone_source.rs`'s `offset = source_anchor − dest_anchor` semantics.
 *  Pure so it can be unit-tested against the Rust formula. */
export function trackedSourcePos(
    anchor: Pt,
    dest: Pt | null,
    cursor: Pt | null,
    anchored: boolean,
): Pt {
    if (anchored || !dest || !cursor) return anchor;
    return { x: anchor.x + (cursor.x - dest.x), y: anchor.y + (cursor.y - dest.y) };
}

/** Record the clone source anchor in the engine (plane / canvas pixels)
 *  and mark that a source now exists so the cursor stops prompting. */
export function setCloneSourceAnchor(cx: number, cy: number): void {
    app.engine?.api.setCloneSource({ x: cx, y: cy });
    hasSource = true;
    sourceAnchor = { x: cx, y: cy };
    refreshCursor();
    rebuildCloneMarker();
    app.requestFrame();
}

/** Re-query whether the active brush needs a source (and its aligned /
 *  anchored mode) whenever the active brush changes. Async (engine
 *  round-trip); the cached result drives arming + the marker. Switching
 *  brushes does not clear the engine anchor — it persists as session state,
 *  so a clone brush reselected mid-session keeps its source. */
function syncNeedsSource(): void {
    const brush = brushGraph.activeBrush ?? null;
    if (brush === lastBrush) return;
    lastBrush = brush;
    const engine = app.engine;
    if (!engine || needsQueryInFlight) return;
    needsQueryInFlight = true;
    Promise.all([engine.api.activeBrushNeedsSource(), engine.api.cloneSourceAnchored()])
        .then(([needs, anchored]: [boolean, boolean]) => {
            needsSource = needs;
            anchoredMode = anchored;
            refreshCursor();
            rebuildCloneMarker();
            app.requestFrame();
        })
        .finally(() => {
            needsQueryInFlight = false;
        });
}

// ---------------------------------------------------------------------------
// On-canvas source marker (persistent 'clone' overlay channel)
// ---------------------------------------------------------------------------

/** Memo key of the last-pushed marker so a static view doesn't re-upload
 *  every frame. Encodes the marker's screen position + kind; `null` means
 *  the channel is currently cleared. */
let lastMarkerKey: string | null = null;

function clearCloneMarker(): void {
    if (lastMarkerKey === null) return;
    app.engine?.api.clearCloneOverlay();
    lastMarkerKey = null;
    app.requestFrame();
}

/** Rebuild the clone marker on the persistent `'clone'` channel: a crosshair
 *  at the (tracked) source when one is set, or a "pick a source" hint near
 *  the cursor when clone is active but unset. Screen-space, so it re-pushes
 *  on view pan/zoom via the per-frame tick — but only when its screen
 *  position actually changed (memoized). */
function rebuildCloneMarker(): void {
    const engine = app.engine;
    const canvasEl = app.canvasEl;
    if (!engine || !canvasEl) return;

    if (!needsSource || !isPaintToolActive()) {
        clearCloneMarker();
        return;
    }

    const b = new OverlayBuilder(canvasEl);
    let key: string;
    if (hasSource && sourceAnchor) {
        const pos = trackedSourcePos(sourceAnchor, destAnchor, cursorPos, anchoredMode);
        const sp = canvasToScreen(pos.x, pos.y, canvasEl);
        key = `src:${Math.round(sp.x)},${Math.round(sp.y)}`;
        b.crosshair([pos.x, pos.y], { color: '#4af', size: 8, gap: 2, thickness: 1.5 });
    } else if (cursorPos) {
        // No source set — an amber prompt at the cursor so a clone brush
        // never silently no-op paints (prior art: GIMP's explicit prompt).
        const sp = canvasToScreen(cursorPos.x, cursorPos.y, canvasEl);
        key = `hint:${Math.round(sp.x)},${Math.round(sp.y)}`;
        b.crosshair([cursorPos.x, cursorPos.y], { color: '#f80', size: 6, gap: 3, thickness: 1.5 });
    } else {
        // Active + unset but no cursor seen yet — nothing to anchor a hint.
        clearCloneMarker();
        return;
    }

    if (key === lastMarkerKey) return;
    lastMarkerKey = key;
    b.push(engine, 'clone');
}

// ---------------------------------------------------------------------------
// Stroke tracking hooks (called by the brush tool)
// ---------------------------------------------------------------------------

/** Clone stroke started at `(cx, cy)` — capture the dest anchor (the engine
 *  captures the same first-dab position) so aligned-mode tracking slides the
 *  source marker with the cursor. No-op unless a clone source is set. */
export function onCloneStrokeStart(cx: number, cy: number): void {
    if (!needsSource) return;
    destAnchor = { x: cx, y: cy };
    cursorPos = { x: cx, y: cy };
    rebuildCloneMarker();
}

/** Clone stroke moved to `(cx, cy)` — update the tracked marker. */
export function onCloneStrokeMove(cx: number, cy: number): void {
    if (!needsSource) return;
    cursorPos = { x: cx, y: cy };
    rebuildCloneMarker();
}

/** Clone stroke ended — drop the dest anchor so the marker snaps back to the
 *  set source (the engine re-anchors `dest` at each stroke's first dab). */
export function onCloneStrokeEnd(): void {
    destAnchor = null;
    rebuildCloneMarker();
}

/** Hover moved to `(cx, cy)` (not painting) — anchors the no-source hint and
 *  keeps the cursor cache warm. */
export function onCloneHoverMove(cx: number, cy: number): void {
    if (!needsSource) return;
    cursorPos = { x: cx, y: cy };
    rebuildCloneMarker();
}

/** Pointer left the canvas — drop the cursor cache. The source crosshair
 *  stays (it's anchored to the set source, not the cursor); the no-source
 *  hint, which follows the cursor, clears. */
export function onCloneHoverLeave(): void {
    cursorPos = null;
    rebuildCloneMarker();
}

// ---------------------------------------------------------------------------
// Arming
// ---------------------------------------------------------------------------

let lastCursorKey: string | null = null;

function isPaintToolActive(): boolean {
    return toolRegistry.get(app.activeToolId)?.group === 'paint';
}

/** The crosshair is shown when the active paint brush needs a source and
 *  either no source is set (a prompt) or the held modifier resolves to
 *  `setCloneSource`. Clone's binding is the most specific, so it always wins
 *  the chord — but reading the one shared resolver means this cursor and the
 *  color picker can never disagree about who owns the modifier. */
function isArmed(): boolean {
    if (!needsSource || !isPaintToolActive()) return false;
    if (!hasSource) return true;
    return dragModifierActions('canvas', heldMods()).has('setCloneSource');
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

/** Per-frame tick — refreshes the needs-source cache on brush change,
 *  re-evaluates the cursor, and rebuilds the on-canvas marker so it tracks
 *  view pan/zoom. Cheap when nothing changed (memo guards on each step). */
export function tickCloneSourceCursor(): void {
    syncNeedsSource();
    refreshCursor();
    rebuildCloneMarker();
}

/** Drop the on-canvas clone marker — called when the paint tool deactivates
 *  so the crosshair can't outlive the tool that owns it. The engine anchor
 *  persists (session state); only the overlay is cleared. */
export function clearCloneSourceCursor(): void {
    clearCloneMarker();
}

let wired = false;

/** Wire the set-source cursor's engagement re-evaluation. Idempotent. Which
 *  modifier arms the crosshair is decided by the shared specificity resolver
 *  (`dragModifierActions` over `heldMods()`), and the held set itself is
 *  owned by `held_mods.ts`, so preset swaps / user overrides flow through
 *  and this cursor can't disagree with the color picker over the modifier. */
export function setupCloneSourceModifierTracking(): void {
    if (wired) return;
    wired = true;

    // Re-evaluate the cursor when the held set changes or a rebind changes
    // the winner (`clickIndex` is rebuilt on config change before this runs).
    onHeldModsChange(refreshCursor);
    config.onChange(refreshCursor);
}
