import { app } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { eyedropperCursor } from './colorpicker_cursor';

// Coordinates the eyedropper cursor's appearance and the modifier-held
// "temporary eyedropper" tool switch. Single source of truth so the
// colorpicker tool, the modifier-held chord, and other paint tools all
// agree on what to render and which tool is active.
//
// The cursor's *armed* state is derived from `app.activeToolId === 'colorpicker'`
// — there is no separate flag. When the user holds the modifier with a
// paint tool active, we actually swap `app.activeToolId` to 'colorpicker'
// (and remember what to restore on release), so the previous tool's real
// `onDeactivate` runs (clearing brush dab overlays, etc.) and the
// colorpicker's `onActivate` runs (taking ownership of the cursor).
// This is the architecturally correct behavior: while the modifier is
// held, the *active tool* really is the eyedropper.

let pressed = false;
let modifierHeld = false;
let pointerDown = false;
let lastKey: string | null = null;

// The tool we soft-switched away from when the modifier engaged. `null`
// when no soft-switch is in flight (either because the modifier isn't
// held, or because the user manually activated the colorpicker).
let previousToolId: string | null = null;

// Marks the brush deactivate / activate pair caused by a modifier-driven
// soft switch (as opposed to a manual hotkey switch). The brush reads
// this in its lifecycle hooks so it can preserve `lastHover` across the
// swap and re-push the hover preview on the way back — avoiding the
// "no dab preview until the next mousemove" gap users complained about.
let softSwitching = false;

export function isSoftSwitching(): boolean {
    return softSwitching;
}

/** Called by `brush.onActivate` after it consumes the soft-switch signal
 *  to re-push its hover preview. */
export function consumeSoftSwitching(): void {
    softSwitching = false;
}

function isPaintToolActive(): boolean {
    return toolRegistry.get(app.activeToolId)?.group === 'paint';
}

function isArmed(): boolean {
    return app.activeToolId === 'colorpicker';
}

function colorKey(): string {
    const fg = app.foreground;
    const bg = app.background;
    return `${pressed ? 'p' : 'i'}|${fg.r},${fg.g},${fg.b}|${bg.r},${bg.g},${bg.b}`;
}

/** Refresh `app.toolCursor` for the current armed state. Cheap via
 *  memoization — only writes when the (pressed, fg, bg) tuple changes. */
function refreshCursor(): void {
    if (!isArmed()) {
        lastKey = null;
        return;
    }
    const key = colorKey();
    if (key === lastKey) return;
    lastKey = key;
    app.toolCursor = eyedropperCursor(app.foreground, app.background, pressed);
}

/** Try to soft-switch into the eyedropper. Bailed if there's no paint
 *  tool to switch from, if the colorpicker is already active, or if the
 *  user is mid-stroke (pointer is down). The pointer-down guard
 *  prevents tearing an in-flight brush stroke; we re-evaluate on
 *  pointerup so a "press Ctrl, finish stroke, release" sequence still
 *  arms after the stroke completes. */
function tryArmViaModifier(): void {
    if (!modifierHeld) return;
    if (pointerDown) return;
    if (app.activeToolId === 'colorpicker') return;
    if (!isPaintToolActive()) return;

    previousToolId = app.activeToolId;
    softSwitching = true;
    app.activeToolId = 'colorpicker';
    app.requestFrame();
    // brush.onDeactivate / colorpicker.onActivate fire via CanvasView's
    // $effect. The brush preserves `lastHover` because `softSwitching`
    // is true; the colorpicker takes over `toolCursor` via its
    // `onActivate` calling `tickEyedropperCursor`.
}

/** Restore the soft-switched tool. Always runs on modifier release so a
 *  mid-Ctrl-held manual tool change still gets its restore eventually
 *  (worst case: the wrong tool is restored if the user hopped around). */
function disarmViaModifier(): void {
    if (previousToolId === null) return;
    const restore = previousToolId;
    previousToolId = null;
    pressed = false;
    // `softSwitching` stays true so brush.onActivate sees it and
    // re-pushes its hover preview. brush clears the flag via
    // `consumeSoftSwitching` after consuming it.
    app.activeToolId = restore;
    app.requestFrame();
}

/** A sample is in progress (mouse button held during pick). Same call
 *  for both the eyedropper tool's pointerdown/up and the modifier-held
 *  chord's handler/release. */
export function setEyedropperPressed(p: boolean): void {
    if (pressed === p) return;
    pressed = p;
    refreshCursor();
}

/** Per-frame tick — picks up foreground updates that `pollPick` commits
 *  between pointer events. Cheap when nothing changed (memo guard). */
export function tickEyedropperCursor(): void {
    refreshCursor();
}

/** True when the eyedropper indicator is currently shown. Kept for
 *  callers that need to gate other behavior on the armed state. */
export function isEyedropperArmed(): boolean {
    return isArmed();
}

let wired = false;

/** Wire global modifier + pointer tracking. Idempotent. The cursor /
 *  tool soft-switch engages as soon as the user *holds* the modifier
 *  with a paint tool active (not just on pointerdown), so the cursor
 *  reflects the upcoming sample action immediately. */
export function setupEyedropperModifierTracking(): void {
    if (wired) return;
    wired = true;

    window.addEventListener('keydown', (e) => {
        if ((e.key === 'Control' || e.key === 'Meta') && !modifierHeld) {
            modifierHeld = true;
            tryArmViaModifier();
        }
    });
    window.addEventListener('keyup', (e) => {
        if ((e.key === 'Control' || e.key === 'Meta') && modifierHeld) {
            modifierHeld = false;
            disarmViaModifier();
        }
    });
    // Window blur (alt-tab, OS focus change) can leave us thinking the
    // modifier is still held when it isn't. Reset on blur.
    window.addEventListener('blur', () => {
        if (modifierHeld) {
            modifierHeld = false;
            disarmViaModifier();
        }
        if (pointerDown) pointerDown = false;
    });

    // Pointer-down tracking so we can refuse to soft-switch mid-stroke
    // (which would orphan a `begin_stroke` without an `end_stroke`).
    // After a stroke finishes (pointerup) we re-evaluate so a "press
    // Ctrl, finish stroke, release pointer" sequence still arms the
    // eyedropper for the next click.
    window.addEventListener('pointerdown', () => { pointerDown = true; });
    window.addEventListener('pointerup', () => {
        pointerDown = false;
        if (modifierHeld) tryArmViaModifier();
    });
    window.addEventListener('pointercancel', () => { pointerDown = false; });
}
