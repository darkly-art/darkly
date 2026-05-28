import { tinykeys } from 'tinykeys';
import { config } from './store.svelte';
import { actions } from '../actions/registry';
import { app } from '../state/app.svelte';
import { activeSiteChain } from '../actions/active_site';
import { toolRegistry } from '../tools/registry';
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
 * Resolve an action's effective keyboard trigger list. The full binding
 * lives in `hotkeys.<id>` under the three-layer config — defaults.yaml +
 * overlay + user override. Multi-binding actions (e.g. `isolateLayer` from
 * `layerThumb:alt+click` + `maskThumb:alt+click`) are joined with `|` in
 * the YAML parser; we split them back into a list here.
 *
 * Empty string means "no keyboard trigger" — used by overlays that
 * explicitly want to disable a binding the previous layer set
 * (e.g. Photoshop sets `hotkeys.isolateLayer = ""`).
 */
export function effectiveHotkeys(actionId: string): string[] {
    const v = config.get(`hotkeys.${actionId}`);
    if (typeof v !== 'string') return [];
    if (!v) return [];
    return v.split('|').filter(Boolean);
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
            const toolGroup = toolRegistry.get(app.activeToolId)?.group ?? null;
            const resolved = resolveChord(entries, chain, toolGroup);
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
