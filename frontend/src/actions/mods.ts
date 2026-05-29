// Cross-platform modifier helpers for the mouse-chord pipeline.
//
// Keyboard hotkeys are handled by `tinykeys`, which expands `$mod` to Ctrl
// on Linux/Win and Cmd on Mac internally. Mouse chords don't go through
// tinykeys — they live in `actions/triggers.ts`'s `clickIndex`. These
// helpers extend the same `$mod`-as-the-platform-primary-modifier
// convention to that path.
//
// The fixed primitive vocabulary is `{ctrl, meta, alt, shift}`. `metaKey`
// is reported honestly (it's Cmd on Mac, Super/Windows on Linux/Win) —
// we never fold it into `ctrl`. Per-platform mapping happens once, at
// chord-index build time, via `substituteMod`.

import { detectPlatform } from '../platform';

const IS_MAC = detectPlatform().os === 'macos';

/** Canonical modifier order. Used wherever a sorted prefix is compared
 *  against an event-derived chord — both must agree on order. */
export const MOD_ORDER: readonly string[] = ['ctrl', 'meta', 'alt', 'shift'];

/** The primitive token `$mod` resolves to on this platform. */
export const MOD_KEY: 'ctrl' | 'meta' = IS_MAC ? 'meta' : 'ctrl';

/** Replace any `$mod` segment in a `+`-joined chord with the platform's
 *  primary-modifier primitive. Pass binding strings through this once at
 *  index-build time so the runtime matcher compares literal-vs-literal. */
export function substituteMod(chord: string): string {
    return chord.split('+').map(p => (p === '$mod' ? MOD_KEY : p)).join('+');
}

/** Apply `substituteMod` to the chord portion of a `[site][@scope]:chord`
 *  binding, leaving the site/scope prefix untouched. */
export function substituteModInBinding(raw: string): string {
    const colonIdx = raw.indexOf(':');
    if (colonIdx < 0) return substituteMod(raw);
    return raw.slice(0, colonIdx + 1) + substituteMod(raw.slice(colonIdx + 1));
}

/** Canonical modifier list from a keyboard, mouse, or pointer event.
 *  Returns the held primitives in `MOD_ORDER`. No fold — `metaKey` is
 *  `'meta'`, not `'ctrl'`. */
export function canonicalModsFromEvent(
    e: { ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean },
): string[] {
    const mods: string[] = [];
    if (e.ctrlKey) mods.push('ctrl');
    if (e.metaKey) mods.push('meta');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');
    return mods;
}

/** True when the event carries the platform's `$mod`-equivalent
 *  (Ctrl on Linux/Win, Cmd on Mac). For one-off checks outside the
 *  chord-index dispatch path. */
export function isModEvent(e: { ctrlKey: boolean; metaKey: boolean }): boolean {
    return IS_MAC ? e.metaKey : e.ctrlKey;
}
