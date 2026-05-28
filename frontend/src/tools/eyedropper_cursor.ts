import { app } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { eyedropperCursor } from './colorpicker_cursor';

// State machine for the eyedropper cursor. The cursor is "armed" whenever
// either the colorpicker tool is active *or* the user is holding the
// modifier key with a paint-group tool active (the `sampleColor` chord
// is then about to fire on click). The cursor's pressed/idle variant is
// driven by whether a sample is currently being taken. One source of
// truth so both entry points (tool + chord) render the same indicator
// and stay in sync with foreground/background changes.

let toolActive = false;
let modifierHeld = false;
let pressed = false;
let armed = false;
let lastKey: string | null = null;

function isPaintToolActive(): boolean {
    return toolRegistry.get(app.activeToolId)?.group === 'paint';
}

function colorKey(): string {
    const fg = app.foreground;
    const bg = app.background;
    return `${pressed ? 'p' : 'i'}|${fg.r},${fg.g},${fg.b}|${bg.r},${bg.g},${bg.b}`;
}

/** Recompute `armed` and update `app.toolCursor`. Idempotent — safe to
 *  call from any state-change entry point or from the per-frame tick. */
function recompute(): void {
    const wantArmed = toolActive || (modifierHeld && isPaintToolActive());
    if (wantArmed !== armed) {
        armed = wantArmed;
        if (!armed) {
            app.toolCursor = null;
            lastKey = null;
            return;
        }
        // Newly armed: clear whatever overlay the active paint tool may
        // have pushed (e.g. the brush dab preview) so it doesn't stay
        // painted on the canvas while the eyedropper cursor takes over.
        // The tool will repush its overlay on the next pointermove
        // after we disarm. Safe no-op when the colorpicker tool is the
        // one arming us — it doesn't push hover overlays.
        app.handle?.clear_overlay();
        // Reset memo so the refresh below actually runs.
        lastKey = null;
    }
    if (!armed) return;
    const key = colorKey();
    if (key === lastKey) return;
    lastKey = key;
    app.toolCursor = eyedropperCursor(app.foreground, app.background, pressed);
}

/** Colorpicker tool entering/leaving its active state. */
export function setEyedropperToolActive(active: boolean): void {
    if (toolActive === active) return;
    toolActive = active;
    if (!active) pressed = false;
    recompute();
}

/** A sample is in progress (mouse button held during pick). Same call
 *  for both the eyedropper tool's pointerdown/up and the modifier-held
 *  chord's handler/release. */
export function setEyedropperPressed(p: boolean): void {
    if (pressed === p) return;
    pressed = p;
    recompute();
}

/** Per-frame tick — picks up foreground updates that `pollPick` commits
 *  between pointer events. Cheap when nothing changed (memo guard). */
export function tickEyedropperCursor(): void {
    recompute();
}

/** True when the eyedropper indicator is currently shown — either the
 *  colorpicker tool is active, or a paint-group tool is active with the
 *  modifier held. CanvasView reads this to gate the active tool's
 *  hover-overlay path so it doesn't race with the eyedropper for
 *  `app.toolCursor` and the GPU overlay buffer. */
export function isEyedropperArmed(): boolean {
    return armed;
}

let wired = false;

/** Wire global modifier-key tracking. Idempotent. The cursor should
 *  appear as soon as the user *holds* the modifier with a paint tool
 *  active (signaling "click here to sample"), not just on pointerdown.
 *  `pointerdown` itself then upgrades the indicator via
 *  `setEyedropperPressed`. */
export function setupEyedropperModifierTracking(): void {
    if (wired) return;
    wired = true;
    window.addEventListener('keydown', (e) => {
        if (e.key === 'Control' || e.key === 'Meta') {
            if (!modifierHeld) {
                modifierHeld = true;
                recompute();
            }
        }
    });
    window.addEventListener('keyup', (e) => {
        if (e.key === 'Control' || e.key === 'Meta') {
            if (modifierHeld) {
                modifierHeld = false;
                // Releasing the modifier also ends any in-flight sample
                // (the chord can no longer be matched without it).
                pressed = false;
                recompute();
            }
        }
    });
    // Window blur (alt-tab, OS focus change) can leave us thinking the
    // modifier is still held when it isn't. Reset on blur.
    window.addEventListener('blur', () => {
        if (modifierHeld || pressed) {
            modifierHeld = false;
            pressed = false;
            recompute();
        }
    });
}
