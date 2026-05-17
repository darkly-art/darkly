import { tinykeys } from 'tinykeys';
import { config } from './store.svelte';
import { actions } from '../actions/registry';
import { app } from '../state/app.svelte';
import { activeSiteChain } from '../actions/active_site';
import {
    parseBinding,
    buildChordIndex,
    resolveChord,
    type ChordEntry,
} from '../actions/hotkey_resolve';

// Re-export the pure helpers so existing import paths (cheatsheet, settings
// widgets) keep resolving. The resolution logic itself lives in
// `actions/hotkey_resolve.ts` so it can be unit-tested without DOM.
export { parseBinding, type ChordEntry };

let cleanup: (() => void) | null = null;

/**
 * Resolve an action's effective keyboard trigger list:
 *   user override (`hotkeys.<id>` in config) ?? action.defaultHotkey ?? [].
 *
 * Empty string is meaningful — it explicitly means "no keyboard trigger" and
 * suppresses any default. Used by presets that disable a default (e.g.
 * Photoshop sets `hotkeys.isolateLayer = ""` to remove Krita's `KeyI`).
 */
export function effectiveHotkeys(actionId: string): string[] {
    const override = config.get(`hotkeys.${actionId}`);
    if (typeof override === 'string') {
        return override ? [override] : [];
    }
    const def = actions.get(actionId)?.defaultHotkey;
    if (!def) return [];
    if (typeof def === 'string') return def ? [def] : [];
    return def.filter(Boolean);
}

/** Single-string view for callers that show one binding per action
 *  (settings UI row, cheatsheet). Returns the first effective binding,
 *  or `""` if none. */
export function effectiveHotkey(actionId: string): string {
    return effectiveHotkeys(actionId)[0] ?? '';
}

/**
 * Register all hotkeys from the action registry + Rust config.
 *
 * For each unique chord across all actions, one tinykeys binding is installed.
 * At dispatch time the handler walks the *priority list* for that chord:
 * scoped entries are tried against the active site chain (innermost-first
 * focus ancestors); the first match dispatches with the site's ctx. A global
 * (no-scope) entry, if present, is the final fallback and dispatches with
 * `{ layerId: app.activeLayerId }`.
 *
 * Call on init and whenever the preset/config changes.
 */
export function registerHotkeys() {
    cleanup?.();

    const chordIndex = buildChordIndex(
        actions.all().map(a => ({ actionId: a.id, bindings: effectiveHotkeys(a.id) })),
    );

    const bindings: Record<string, (e: KeyboardEvent) => void> = {};

    for (const [chord, entries] of chordIndex) {
        bindings[chord] = (e: KeyboardEvent) => {
            // Suppress global hotkeys while a modal dialog is open so the
            // dialog's own keys (Esc to close, etc.) work and modal-scoped
            // shortcuts don't leak to the canvas.
            if (document.querySelector('dialog[open]')) return;
            const el = e.target as HTMLElement;
            const tag = el?.tagName;
            // Allow hotkeys through range sliders — they don't need text input.
            if (tag === 'INPUT' && (el as HTMLInputElement).type !== 'range') return;
            if (tag === 'TEXTAREA' || tag === 'SELECT') return;

            const chain = activeSiteChain();
            const resolved = resolveChord(entries, chain);
            if (!resolved) return;
            e.preventDefault();
            const ctx = resolved.site
                ? resolved.site.ctx(e)
                : { layerId: app.activeLayerId ?? undefined };
            actions.dispatch(resolved.entry.actionId, ctx);
        };
    }

    cleanup = tinykeys(window, bindings);
}

export function unregisterHotkeys() {
    cleanup?.();
    cleanup = null;
}
