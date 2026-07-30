import { getActiveInstance, type DarklyInstance } from '../state/app.svelte';
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
// The engine-backed mirror (source anchor, needs-source cache, per-stroke
// tracking) is per-document, so it's keyed per `DarklyInstance` — switching
// tabs shows each tab's own clone source, and one tab's stroke never drives
// another's marker. The engagement state itself is a pointer singleton (one
// pointer, one modifier chord) and stays module-global, always evaluated
// against the *focused* instance.

// ---------------------------------------------------------------------------
// Per-instance engine-backed anchor + needs-source cache
// ---------------------------------------------------------------------------

interface Pt {
    x: number;
    y: number;
}

/** Per-document mirror of the engine's clone state + on-canvas marker memo. */
interface CloneState {
    hasSource: boolean;
    needsSource: boolean;
    /** Aligned (false) vs anchored (true), cached from the engine alongside
     *  `needsSource`. Drives the on-canvas marker's stroke tracking. */
    anchoredMode: boolean;
    /** The set source anchor in canvas / plane pixels — a local mirror of the
     *  engine's `clone_source_anchor` (the engine exposes no getter). */
    sourceAnchor: Pt | null;
    /** Destination anchor captured at `pointerdown` (canvas pixels), mirroring
     *  the engine's per-stroke first-dab `dest_anchor`. Non-null only while a
     *  clone stroke is in flight. */
    destAnchor: Pt | null;
    /** Live dab centre while a clone stroke is in flight; null otherwise. */
    cursorPos: Pt | null;
    lastBrush: string | null | undefined;
    needsQueryInFlight: boolean;
    /** Memo key of the last-pushed marker so a static view doesn't re-upload
     *  every frame; `null` means the channel is currently cleared. */
    lastMarkerKey: string | null;
}

const states = new WeakMap<DarklyInstance, CloneState>();

function stateFor(inst: DarklyInstance): CloneState {
    let s = states.get(inst);
    if (!s) {
        s = {
            hasSource: false,
            needsSource: false,
            anchoredMode: false,
            sourceAnchor: null,
            destAnchor: null,
            cursorPos: null,
            lastBrush: undefined,
            needsQueryInFlight: false,
            lastMarkerKey: null,
        };
        states.set(inst, s);
    }
    return s;
}

/** The focused instance's clone state, or null when there's no focused
 *  instance. */
function focusedState(): { inst: DarklyInstance; st: CloneState } | null {
    const inst = getActiveInstance();
    if (!inst) return null;
    return { inst, st: stateFor(inst) };
}

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

/** Record the clone source anchor in the (focused) engine (plane / canvas
 *  pixels), pinning the currently active layer as the clone source (null =
 *  clone from the painted layer), and mark that a source now exists so the
 *  marker can draw at it. */
export function setCloneSourceAnchor(cx: number, cy: number): void {
    const f = focusedState();
    if (!f) return;
    f.inst.engine?.api.setCloneSource({ x: cx, y: cy, layer: f.inst.activeLayerId });
    f.st.hasSource = true;
    f.st.sourceAnchor = { x: cx, y: cy };
    refreshEngagement();
    rebuildCloneMarker(f.inst, f.st);
    f.inst.requestFrame();
}

/** Re-query whether the active brush needs a source (and its aligned /
 *  anchored mode) whenever the active brush changes. Async (engine
 *  round-trip); the cached result drives arming + the marker. Switching
 *  brushes does not clear the engine anchor — it persists as session state,
 *  so a clone brush reselected mid-session keeps its source. */
function syncNeedsSource(inst: DarklyInstance, st: CloneState): void {
    const brush = brushGraph.activeBrush ?? null;
    if (brush === st.lastBrush) return;
    st.lastBrush = brush;
    const engine = inst.engine;
    if (!engine || st.needsQueryInFlight) return;
    st.needsQueryInFlight = true;
    Promise.all([engine.api.activeBrushNeedsSource(), engine.api.cloneSourceAnchored()])
        .then(([needs, anchored]: [boolean, boolean]) => {
            st.needsSource = needs;
            st.anchoredMode = anchored;
            refreshEngagement();
            rebuildCloneMarker(inst, st);
            inst.requestFrame();
        })
        .finally(() => {
            st.needsQueryInFlight = false;
        });
}

// ---------------------------------------------------------------------------
// On-canvas source marker (persistent 'clone' overlay channel)
// ---------------------------------------------------------------------------

function clearCloneMarker(inst: DarklyInstance, st: CloneState): void {
    if (st.lastMarkerKey === null) return;
    inst.engine?.api.clearCloneOverlay();
    st.lastMarkerKey = null;
    inst.requestFrame();
}

/** Rebuild the clone marker on the persistent `'clone'` channel: a crosshair
 *  at the (tracked) source when one is set. Screen-space, so it re-pushes
 *  on view pan/zoom via the per-frame tick — but only when its screen
 *  position actually changed (memoized). */
function rebuildCloneMarker(inst: DarklyInstance, st: CloneState): void {
    const engine = inst.engine;
    const canvasEl = inst.canvasEl;
    if (!engine || !canvasEl) return;

    if (!st.needsSource || !isPaintToolActive() || !st.hasSource || !st.sourceAnchor) {
        clearCloneMarker(inst, st);
        return;
    }

    const pos = trackedSourcePos(st.sourceAnchor, st.destAnchor, st.cursorPos, st.anchoredMode);
    const sp = canvasToScreen(pos.x, pos.y, canvasEl);
    const key = `src:${Math.round(sp.x)},${Math.round(sp.y)}`;
    if (key === st.lastMarkerKey) return;
    st.lastMarkerKey = key;
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
    const f = focusedState();
    if (!f || !f.st.needsSource) return;
    f.st.destAnchor = { x: cx, y: cy };
    f.st.cursorPos = { x: cx, y: cy };
    rebuildCloneMarker(f.inst, f.st);
}

/** Clone stroke moved to `(cx, cy)` — update the tracked marker. */
export function onCloneStrokeMove(cx: number, cy: number): void {
    const f = focusedState();
    if (!f || !f.st.needsSource) return;
    f.st.cursorPos = { x: cx, y: cy };
    rebuildCloneMarker(f.inst, f.st);
}

/** Clone stroke ended — drop the dest anchor (the engine re-anchors `dest`
 *  at each stroke's first dab) and the dab centre with it, so the marker
 *  snaps back to the set source. */
export function onCloneStrokeEnd(): void {
    const f = focusedState();
    if (!f) return;
    f.st.destAnchor = null;
    f.st.cursorPos = null;
    rebuildCloneMarker(f.inst, f.st);
}

// ---------------------------------------------------------------------------
// Arming
// ---------------------------------------------------------------------------

let engaged = false;

function isPaintToolActive(): boolean {
    const id = getActiveInstance()?.activeToolId;
    return id != null && toolRegistry.get(id)?.group === 'paint';
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
 *  releases both and restores the hover immediately. Always evaluated against
 *  the focused instance's needs-source cache (the pointer singleton). */
function refreshEngagement(): void {
    const needs = focusedState()?.st.needsSource ?? false;
    if (engaged) {
        // Staying engaged ignores pointer state (a set-source drag in
        // flight stays armed until the modifier lifts).
        if (!cloneEngages(
            dragModifierActions('canvas', heldMods()),
            isPaintToolActive(), needs, false,
        )) {
            engaged = false;
            disengageModifierCursor('cloneSource');
        }
    } else if (cloneEngages(
        dragModifierActions('canvas', heldMods()),
        isPaintToolActive(), needs, isPointerDown(),
    )) {
        engaged = true;
        engageModifierCursor('cloneSource', 'crosshair');
    }
}

/** Per-frame tick — refreshes the (focused instance's) needs-source cache on
 *  brush change, re-evaluates engagement, and rebuilds the on-canvas marker so
 *  it tracks view pan/zoom. Cheap when nothing changed (memo guards on each
 *  step). Called only from the focused instance's frame loop. */
export function tickCloneSourceCursor(): void {
    const f = focusedState();
    if (!f) return;
    syncNeedsSource(f.inst, f.st);
    refreshEngagement();
    rebuildCloneMarker(f.inst, f.st);
}

/** Drop the on-canvas clone marker and any engagement — called when the
 *  paint tool deactivates so neither the crosshair nor the hover
 *  suppression can outlive the tool that owns them. The engine anchor
 *  persists (session state); only the overlay + arming are cleared. */
export function clearCloneSourceCursor(): void {
    const f = focusedState();
    if (f) clearCloneMarker(f.inst, f.st);
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
