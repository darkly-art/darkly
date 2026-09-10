import { tinykeys } from 'tinykeys';
import { effectiveHotkeys } from './store.svelte';
import { actions } from '../actions/registry';
import { app } from '../state/app.svelte';
import { activeSiteChain } from '../actions/active_site';
import { toolRegistry } from '../tools/registry';
import { brushGraph } from '../state/brush_graph.svelte';
import { isEditableTarget } from '../lib/isEditableTarget';
import {
    parseBinding,
    buildChordIndex,
    resolveChord,
    type ChordEntry,
} from '../actions/hotkey_resolve';

// Re-export the pure helpers so existing import paths (cheatsheet, settings
// widgets) keep resolving. Chord resolution lives in
// `actions/hotkey_resolve.ts` so it can be unit-tested without DOM; the
// config reads live on the config store.
export { parseBinding, type ChordEntry };
export { effectiveHotkeys, effectiveHotkey } from './store.svelte';

let cleanup: (() => void) | null = null;

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
            // Keys typed into a text field are content, not shortcuts (range
            // sliders excepted; see `isEditableTarget`).
            if (isEditableTarget(e.target)) return;

            const chain = activeSiteChain();
            const toolGroup = toolRegistry.get(app.activeToolId)?.group ?? null;
            const activeBrush = brushGraph.activeBrush?.toLowerCase() ?? null;
            const resolved = resolveChord(entries, chain, toolGroup, activeBrush);
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
