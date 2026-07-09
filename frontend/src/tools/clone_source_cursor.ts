import { app } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { brushGraph } from '../state/brush_graph.svelte';
import { dragModifierActions } from '../actions/triggers';
import { heldMods, onHeldModsChange } from '../actions/held_mods';
import { config } from '../config/store.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import { canvasToScreen } from '../canvas/coordinates';
import {
    chordCursorEngages,
    engageModifierCursor,
    disengageModifierCursor,
    isPointerDown,
    onPointerRelease,
} from './modifier_cursor';

// Clone set-source cursor + arming. The arming modifier is derived at
// runtime from whatever `setCloneSource` is bound to in the active config
// layer (so preset swaps + user overrides flow through), and a crosshair
// cursor confirms sample mode while the chord is held. Hover suppression,
// the `app.toolCursor` slot, and the suspend/restore handoff to the active
// tool are owned by the shared engagement machinery in `modifier_cursor.ts`
// — while armed, the brush's dab preview is suspended so it can't fight the
// crosshair, and disarming restores it immediately.
//
// Armed condition (crosshair shown): a paint tool is active, the active
// brush needs a source (`activeBrushNeedsSource`, cached async), and the
// held modifier resolves to `setCloneSource` — a transient sample-mode
// indicator, exactly like the color picker's dropper. Otherwise the dab
// preview is the default state, source or no source (with no source set,
// the preview renders in neutral grey and the Rust no-op gate skips the
// stroke).
//
// The gesture itself is dispatched by the brush-scoped chord binding
// through `dispatchDrag`; this module only owns the arming decision, the
// cached "needs source" flag, and the on-canvas source marker.

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
/** Live dab centre while a clone stroke is in flight (owned by the
 *  `onCloneStroke*` hooks); null otherwise. Drives the aligned-mode
 *  source-marker tracking. */
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

/** Record the clone source anchor in the engine (plane / canvas pixels),
 *  pinning the currently active layer as the clone source (null = clone
 *  from the painted layer), and mark that a source now exists so the
 *  marker can draw at it. */
export function setCloneSourceAnchor(cx: number, cy: number): void {
    app.engine?.api.setCloneSource({ x: cx, y: cy, layer: app.activeLayerId });
    hasSource = true;
    sourceAnchor = { x: cx, y: cy };
    refreshEngagement();
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
            refreshEngagement();
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
 *  at the (tracked) source when one is set. Screen-space, so it re-pushes
 *  on view pan/zoom via the per-frame tick — but only when its screen
 *  position actually changed (memoized). */
function rebuildCloneMarker(): void {
    const engine = app.engine;
    const canvasEl = app.canvasEl;
    if (!engine || !canvasEl) return;

    if (!needsSource || !isPaintToolActive() || !hasSource || !sourceAnchor) {
        clearCloneMarker();
        return;
    }

    const pos = trackedSourcePos(sourceAnchor, destAnchor, cursorPos, anchoredMode);
    const sp = canvasToScreen(pos.x, pos.y, canvasEl);
    const key = `src:${Math.round(sp.x)},${Math.round(sp.y)}`;
    if (key === lastMarkerKey) return;
    lastMarkerKey = key;
    const b = new OverlayBuilder(canvasEl);
    // Snapshot-invert arms so the marker stays legible over any content.
    b.crosshair([pos.x, pos.y], { invert: true, size: 8, gap: 2, thickness: 1.5 });
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

/** Clone stroke ended — drop the dest anchor (the engine re-anchors `dest`
 *  at each stroke's first dab) and the dab centre with it, so the marker
 *  snaps back to the set source. */
export function onCloneStrokeEnd(): void {
    destAnchor = null;
    cursorPos = null;
    rebuildCloneMarker();
}

// ---------------------------------------------------------------------------
// Arming
// ---------------------------------------------------------------------------

let engaged = false;

function isPaintToolActive(): boolean {
    return toolRegistry.get(app.activeToolId)?.group === 'paint';
}

/** Pure engagement decision: the crosshair arms while the active paint brush
 *  needs a source and the held modifier resolves to `setCloneSource`. Clone's
 *  binding is the most specific, so it always wins the chord — but reading
 *  the one shared resolver means this cursor and the color picker can never
 *  disagree about who owns the modifier. `pointerDown` blocks a *first*
 *  engagement only — suppressing the hover mid-stroke would freeze the
 *  stroke's dispatch; staying engaged never consults it. Split out so the
 *  decision is unit-testable without the DOM state machine. */
export function cloneEngages(
    resolved: Set<string>,
    paintToolActive: boolean,
    needsSource: boolean,
    pointerDown: boolean,
): boolean {
    return needsSource &&
        chordCursorEngages(resolved, paintToolActive, pointerDown, 'setCloneSource');
}

/** Re-check engagement and drive the arm/disarm transitions through the
 *  shared machinery: arming suspends the active tool's hover (so the dab
 *  preview can't stomp the crosshair) and takes the cursor slot; disarming
 *  releases both and restores the hover immediately. */
function refreshEngagement(): void {
    if (engaged) {
        // Staying engaged ignores pointer state (a set-source drag in
        // flight stays armed until the modifier lifts).
        if (!cloneEngages(
            dragModifierActions('canvas', heldMods()),
            isPaintToolActive(), needsSource, false,
        )) {
            engaged = false;
            disengageModifierCursor('cloneSource');
        }
    } else if (cloneEngages(
        dragModifierActions('canvas', heldMods()),
        isPaintToolActive(), needsSource, isPointerDown(),
    )) {
        engaged = true;
        engageModifierCursor('cloneSource', 'crosshair');
    }
}

/** Per-frame tick — refreshes the needs-source cache on brush change,
 *  re-evaluates engagement, and rebuilds the on-canvas marker so it tracks
 *  view pan/zoom. Cheap when nothing changed (memo guards on each step). */
export function tickCloneSourceCursor(): void {
    syncNeedsSource();
    refreshEngagement();
    rebuildCloneMarker();
}

/** Drop the on-canvas clone marker and any engagement — called when the
 *  paint tool deactivates so neither the crosshair nor the hover
 *  suppression can outlive the tool that owns them. The engine anchor
 *  persists (session state); only the overlay + arming are cleared. */
export function clearCloneSourceCursor(): void {
    clearCloneMarker();
    if (engaged) {
        engaged = false;
        disengageModifierCursor('cloneSource');
    }
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

    // Re-evaluate when the held set changes or a rebind changes the winner
    // (`clickIndex` is rebuilt on config change before this runs), and on
    // pointer release — the first-engage gate re-opens after a stroke.
    onHeldModsChange(refreshEngagement);
    config.onChange(refreshEngagement);
    onPointerRelease(refreshEngagement);
}
