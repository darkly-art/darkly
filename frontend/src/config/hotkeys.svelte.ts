import { tinykeys } from 'tinykeys';
import { config } from './store.svelte';
import { actions } from '../actions/registry';
import { app } from '../state/app.svelte';

let cleanup: (() => void) | null = null;

/**
 * Resolve an action's effective keyboard trigger:
 *   user override (`hotkeys.<id>` in config) ?? action.defaultHotkey ?? unbound.
 *
 * Empty string is meaningful — it explicitly means "no keyboard trigger" and
 * suppresses any default. Used by presets that disable a default (e.g.
 * Photoshop sets `hotkeys.isolateLayer = ""` to remove Krita's `KeyI`).
 */
export function effectiveHotkey(actionId: string): string {
    const override = config.get(`hotkeys.${actionId}`);
    if (typeof override === 'string') return override;
    const action = actions.get(actionId);
    return action?.defaultHotkey ?? '';
}

/**
 * Register all hotkeys from the action registry + Rust config.
 * Call on init and whenever the preset/config changes.
 */
export function registerHotkeys() {
    cleanup?.();

    const bindings: Record<string, (e: KeyboardEvent) => void> = {};

    for (const action of actions.all()) {
        const key = effectiveHotkey(action.id);
        if (!key) continue;

        bindings[key] = (e: KeyboardEvent) => {
            // Suppress global hotkeys while a modal dialog is open so the
            // dialog's own keys (Esc to close, etc.) work and modal-scoped
            // shortcuts don't leak to the canvas.
            if (document.querySelector('dialog[open]')) return;
            const el = e.target as HTMLElement;
            const tag = el?.tagName;
            // Allow hotkeys through range sliders — they don't need text input.
            if (tag === 'INPUT' && (el as HTMLInputElement).type !== 'range') return;
            if (tag === 'TEXTAREA' || tag === 'SELECT') return;
            e.preventDefault();
            actions.dispatch(action.id, { layerId: app.activeLayerId ?? undefined });
        };
    }

    cleanup = tinykeys(window, bindings);
}

export function unregisterHotkeys() {
    cleanup?.();
    cleanup = null;
}
