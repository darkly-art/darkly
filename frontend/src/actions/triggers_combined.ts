// Unified read/write model for the Settings UI. Each action stores its
// triggers in two split namespaces — `hotkeys.<id>` for keyboard chords
// (dispatched by tinykeys) and `mouseclicks.<id>` for mouse chords
// (dispatched by `dispatchClick`/`dispatchDrag`). The Hotkeys tab treats
// them as one flat list per action; this module bridges the two.

import { config } from '../config/store.svelte';
import { parseBinding } from './hotkey_resolve';
import { substituteModInBinding } from './mods';
import { actions } from './registry';

export type TriggerKind = 'kbd' | 'mouse';

export interface Trigger {
    kind: TriggerKind;
    /** Full `[site:]chord` binding string. May be `""` for a freshly-added
     *  row the user hasn't captured into yet. */
    binding: string;
}

// ---- Pure helpers (vitest-friendly, no config dependency) ----

function splitList(raw: string | undefined): string[] {
    if (typeof raw !== 'string' || !raw) return [];
    return raw.split('|').filter(Boolean);
}

export function parseTriggerStrings(
    kbdRaw: string | undefined,
    mouseRaw: string | undefined,
): Trigger[] {
    return [
        ...splitList(kbdRaw).map(b => ({ kind: 'kbd' as const, binding: b })),
        ...splitList(mouseRaw).map(b => ({ kind: 'mouse' as const, binding: b })),
    ];
}

export function serializeTriggers(triggers: Trigger[]): { kbd: string; mouse: string } {
    // Drop rows with an empty chord — including ones that only carry a
    // `<site>:` prefix (a freshly-added row the user didn't capture into
    // before navigating away). Those would persist as ghost entries that
    // dispatch to nothing.
    const isBound = (t: Trigger) => !!t.binding && !!parseBinding(t.binding).chord;
    return {
        kbd: triggers.filter(t => t.kind === 'kbd' && isBound(t)).map(t => t.binding).join('|'),
        mouse: triggers.filter(t => t.kind === 'mouse' && isBound(t)).map(t => t.binding).join('|'),
    };
}

/** Vocabulary terminals that identify a mouse chord. Anything whose last
 *  segment matches one of these is a mouse chord; everything else is keyboard.
 *  Keeping this set tight (terminal verbs only) means a key like `KeyM` —
 *  which contains no `+` segment matching — is unambiguously keyboard. */
const MOUSE_VERBS = new Set([
    'click', 'doubleClick', 'middleClick',
    'drag', 'middleDrag', 'rightDrag',
]);

/** Classify a bare chord (no site prefix) as keyboard or mouse. */
export function detectKind(chord: string): TriggerKind {
    if (!chord) return 'kbd';
    const last = chord.split('+').pop() ?? '';
    return MOUSE_VERBS.has(last) ? 'mouse' : 'kbd';
}

// ---- Config-bound API (used by the Settings UI) ----

export function readTriggers(actionId: string): Trigger[] {
    return parseTriggerStrings(
        config.get(`hotkeys.${actionId}`) as string | undefined,
        config.get(`mouseclicks.${actionId}`) as string | undefined,
    );
}

export function writeTriggers(actionId: string, triggers: Trigger[]): void {
    const { kbd, mouse } = serializeTriggers(triggers);
    config.set(`hotkeys.${actionId}`, kbd);
    config.set(`mouseclicks.${actionId}`, mouse);
}

/** Drop user overrides for both namespaces — falls back to overlay/default. */
export function resetTriggers(actionId: string): void {
    config.resetKey(`hotkeys.${actionId}`);
    config.resetKey(`mouseclicks.${actionId}`);
}

/** True if either namespace has a user-layer override. */
export function hasTriggerOverride(actionId: string): boolean {
    return (
        config.hasOverride(`hotkeys.${actionId}`)
        || config.hasOverride(`mouseclicks.${actionId}`)
    );
}

/** Find actions whose effective bindings collide with `binding`, excluding
 *  `selfActionId`. Compares after `$mod` substitution so `$mod+KeyZ` and
 *  `ctrl+KeyZ` are treated as the same chord on Linux/Win. Returns the
 *  colliders' display names. */
export function findTriggerConflicts(
    binding: string,
    selfActionId: string,
): string[] {
    if (!binding) return [];
    const target = substituteModInBinding(binding);
    const colliders: string[] = [];
    for (const a of actions.all()) {
        if (a.id === selfActionId) continue;
        const kbd = splitList(config.get(`hotkeys.${a.id}`) as string | undefined);
        const mouse = splitList(config.get(`mouseclicks.${a.id}`) as string | undefined);
        for (const b of [...kbd, ...mouse]) {
            if (substituteModInBinding(b) === target) {
                colliders.push(a.displayName);
                break;
            }
        }
    }
    return colliders;
}

/** Convenience: parse the site portion (or `null` for global) out of a
 *  binding string. Thin wrapper around `parseBinding` for callers that only
 *  care about scope. */
export function siteOf(binding: string): string | null {
    return parseBinding(binding).site;
}

/** Convenience: parse the chord portion (without site/scope prefix). */
export function chordOf(binding: string): string {
    return parseBinding(binding).chord;
}
