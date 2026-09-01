import { canonicalModsFromEvent } from './mods';

// Canonical held-modifier tracking, shared by every cursor / overlay that
// arms off a held modifier (the color picker's dropper, the clone
// set-source crosshair, …). One window-level listener set owns the truth so
// the arming machinery never disagrees about what's held.
//
// The string is a canonical `+`-joined modifier list (e.g. `""`, `"ctrl"`,
// `"ctrl+alt"`) in `MOD_ORDER`, matching what `canonicalModsFromEvent`
// reports off a real event, so it compares literal-vs-literal against the
// `$mod`-substituted chords the trigger index resolves.

let held = '';
const listeners = new Set<() => void>();

/** The currently-held modifier set as a canonical `+`-joined string. */
export function heldMods(): string {
    return held;
}

/** Subscribe to held-modifier changes. Returns an unsubscribe function. */
export function onHeldModsChange(cb: () => void): () => void {
    listeners.add(cb);
    return () => listeners.delete(cb);
}

function set(next: string): void {
    if (next === held) return;
    held = next;
    for (const cb of listeners) cb();
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

/** Wire global modifier tracking. Idempotent. Any key/pointer event carries
 *  the full modifier state, so we read it off the event rather than tracking
 *  individual physical keys; `blur` (alt-tab / OS focus loss) resets a
 *  possibly-stranded held set. */
export function setupHeldModsTracking(): void {
    if (wired) return;
    wired = true;

    // Any key event (not just the modifier keys) carries the full modifier
    // state: read it off the event so we never map physical keys ourselves.
    const onKey = (e: KeyboardEvent) => set(modsFromEvent(e));
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKey);

    // Window blur can strand modifier state at "held" when the OS swallows
    // the corresponding key-up. Reset to nothing held.
    window.addEventListener('blur', () => set(''));

    // Pointer events also expose modifier state: pick up drift if a
    // keydown/keyup was swallowed (focus changes can lose them).
    window.addEventListener('pointermove', (e) => set(modsFromEvent(e)));
}
