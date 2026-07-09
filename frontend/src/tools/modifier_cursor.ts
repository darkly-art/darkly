import { app } from '../state/app.svelte';
import { toolRegistry, type ToolContext } from './registry';
import { toolEngine } from './tool_session';
import { screenToCanvas } from '../canvas/coordinates';

// Shared machinery for modifier-held cursors — chords that temporarily own
// the pointer pipeline over the active tool (the color picker's dropper,
// the clone brush's set-source crosshair, …). Each such cursor module keeps
// its own arming decision (which chord, which preconditions) and registers
// here when it engages. This module owns everything the engagers would
// otherwise have to duplicate — and everything they must not fight over:
//
// - **Hover suppression.** While any cursor is engaged, CanvasView skips the
//   active tool's hover dispatch (`isToolHoverSuppressed`), so e.g. the
//   brush's dab preview can't stomp the engaged cursor.
// - **The `app.toolCursor` slot.** Engagers register their cursor value;
//   the most recently engaged wins, and a disengage re-asserts the
//   surviving engager instead of leaving a last-writer-wins hole.
// - **Suspend / restore handoff.** First engage tears down the active
//   tool's hover feedback (`tool.suspendHover`, falling back to a generic
//   overlay clear); the final disengage hands the pipeline back
//   (`tool.restoreHover` at the last tracked pointer position).
// - **Window-level pointer truth.** One listener set tracks the
//   pointer-down gate (engagers refuse to *first*-engage mid-stroke) and
//   the last on-canvas pointer position.

/** Engaged cursors, in engagement order — the last entry owns the slot. */
const engagers = new Map<string, string>();

let pointerDown = false;
/** Latest pointer position in canvas coordinates while over the canvas;
 *  null when off-canvas, so a disengage outside the canvas doesn't
 *  spuriously re-establish a hover overlay. */
let lastCanvas: { x: number; y: number } | null = null;
/** A final disengage happened while a pointer was down — restoring the
 *  hover mid-drag would draw a preview under the gesture, so it waits for
 *  the release. Cancelled by a re-engagement. */
let restorePending = false;

const releaseListeners = new Set<() => void>();

/** True while any modifier cursor is engaged. CanvasView suppresses the
 *  active tool's hover dispatch while this holds. */
export function isToolHoverSuppressed(): boolean {
    return engagers.size > 0;
}

/** Whether any pointer is currently down (window-level truth). Engagers use
 *  this to refuse a first engagement mid-stroke. */
export function isPointerDown(): boolean {
    return pointerDown;
}

/** Last tracked pointer position in canvas coordinates, or null when the
 *  pointer is off-canvas. */
export function lastCanvasPos(): { x: number; y: number } | null {
    return lastCanvas;
}

/** Subscribe to pointer releases (the engagement gate re-opening). Returns
 *  an unsubscribe function. */
export function onPointerRelease(cb: () => void): () => void {
    releaseListeners.add(cb);
    return () => releaseListeners.delete(cb);
}

/** The context handed to `suspendHover` / `restoreHover`, or null when no
 *  tool session / canvas is live. */
function hoverCtx(): ToolContext | null {
    const engine = toolEngine();
    const canvasEl = app.canvasEl;
    if (!engine || !canvasEl) return null;
    return {
        engine,
        canvasEl,
        screenToCanvas: (sx, sy) => screenToCanvas(sx, sy, canvasEl),
    };
}

function suspendActiveToolHover(): void {
    const tool = toolRegistry.get(app.activeToolId);
    const ctx = hoverCtx();
    if (tool?.suspendHover && ctx) {
        tool.suspendHover(ctx);
    } else {
        // Tools without the hook still get their transient overlay cleared —
        // `clear_overlay` is generic; we don't know which tool drew it.
        app.engine?.api.clearOverlay();
    }
}

function restoreActiveToolHover(): void {
    const tool = toolRegistry.get(app.activeToolId);
    const ctx = hoverCtx();
    if (tool?.restoreHover && ctx && lastCanvas) {
        tool.restoreHover(ctx, lastCanvas.x, lastCanvas.y);
    }
}

/** The current slot owner — the most recently engaged id (Map iterates in
 *  insertion order) — or undefined when nothing is engaged. */
function winnerId(): string | undefined {
    let last: string | undefined;
    for (const id of engagers.keys()) last = id;
    return last;
}

/** Engage `id` with the given CSS cursor value. The first engagement
 *  overall suspends the active tool's hover; the newest engagement owns
 *  the cursor slot. Re-engaging an already-engaged id just updates its
 *  cursor (without promoting it to owner). */
export function engageModifierCursor(id: string, cursor: string): void {
    if (engagers.has(id)) {
        updateModifierCursor(id, cursor);
        return;
    }
    if (engagers.size === 0) suspendActiveToolHover();
    restorePending = false;
    engagers.set(id, cursor);
    app.toolCursor = cursor;
    app.requestFrame();
}

/** Update an engaged cursor's value (e.g. the picker's color ring tracking
 *  a live pick). Writes the slot only if `id` currently owns it. */
export function updateModifierCursor(id: string, cursor: string): void {
    if (!engagers.has(id)) return;
    engagers.set(id, cursor);
    if (winnerId() === id) app.toolCursor = cursor;
}

/** Disengage `id`. A surviving engager re-takes the cursor slot; the final
 *  disengage releases the slot and hands the hover pipeline back to the
 *  active tool. `release: false` skips that handoff — for an engager that
 *  knows a new owner is taking the cursor directly (e.g. the picker chord
 *  dissolving because the color-picker *tool* just became active). */
export function disengageModifierCursor(
    id: string,
    opts?: { release?: boolean },
): void {
    if (!engagers.delete(id)) return;
    const survivor = winnerId();
    if (survivor !== undefined) {
        app.toolCursor = engagers.get(survivor)!;
    } else if (opts?.release !== false) {
        app.toolCursor = null;
        if (pointerDown) {
            restorePending = true;
        } else {
            restoreActiveToolHover();
        }
    }
    app.requestFrame();
}

// ---------------------------------------------------------------------------
// Window-level pointer tracking
// ---------------------------------------------------------------------------

/** Record a pointer press (gates first-engagement). */
export function notePointerDown(): void {
    pointerDown = true;
}

/** Record a pointer release: notify engagement re-evaluators first (a
 *  re-engagement cancels the pending restore), then perform a restore
 *  deferred from a mid-drag disengage. */
export function notePointerUp(): void {
    pointerDown = false;
    for (const cb of releaseListeners) cb();
    if (restorePending && engagers.size === 0) {
        restorePending = false;
        restoreActiveToolHover();
    }
}

/** Track the latest canvas-relative pointer position. Window-level so it
 *  keeps updating while CanvasView suppresses the active tool's dispatch. */
export function trackPointer(e: { clientX: number; clientY: number }): void {
    const canvasEl = app.canvasEl;
    if (!canvasEl) {
        lastCanvas = null;
        return;
    }
    const rect = canvasEl.getBoundingClientRect();
    if (
        e.clientX < rect.left || e.clientX > rect.right ||
        e.clientY < rect.top || e.clientY > rect.bottom
    ) {
        lastCanvas = null;
        return;
    }
    lastCanvas = screenToCanvas(e.clientX, e.clientY, canvasEl);
}

let wired = false;

/** Wire the window-level pointer tracking. Idempotent. `blur` and
 *  `pointercancel` count as releases — the OS can swallow the matching
 *  pointer-up, and a stranded "down" would wedge engagement forever. */
export function setupModifierCursorTracking(): void {
    if (wired) return;
    wired = true;

    window.addEventListener('pointerdown', notePointerDown);
    window.addEventListener('pointerup', notePointerUp);
    window.addEventListener('pointercancel', notePointerUp);
    window.addEventListener('blur', notePointerUp);
    window.addEventListener('pointermove', trackPointer);
    // Leaving the window produces no further moves — drop the tracked
    // position so a cursor-following consumer (e.g. the clone brush's
    // no-source hint) doesn't stick at the last seen edge position.
    window.addEventListener('pointerout', (e) => {
        if (!e.relatedTarget) lastCanvas = null;
    });
}
