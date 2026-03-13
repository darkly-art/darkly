import { tinykeys } from 'tinykeys';
import { config } from './store.svelte';
import { actions } from '../actions/registry';
import { app } from '../state/app.svelte';
import { validateBindings } from '../actions/validation';

let cleanup: (() => void) | null = null;

/**
 * Register all hotkeys from the action registry + Rust config.
 * Call on init and whenever the preset changes.
 * Iterates registered action IDs, looks up keyboard bindings from config,
 * and dispatches to the action registry.
 */
export function registerHotkeys() {
    cleanup?.();

    // Validate all bindings — logs warnings for conflicts and context mismatches
    validateBindings((k) => config.get(k));

    const bindings: Record<string, (e: KeyboardEvent) => void> = {};

    for (const id of actions.ids()) {
        const key = config.get(`hotkeys.${id}`) as string | undefined;
        if (!key || typeof key !== 'string') continue;

        bindings[key] = (e: KeyboardEvent) => {
            const el = e.target as HTMLElement;
            const tag = el?.tagName;
            // Allow hotkeys through range sliders — they don't need text input.
            if (tag === 'INPUT' && (el as HTMLInputElement).type !== 'range') return;
            if (tag === 'TEXTAREA' || tag === 'SELECT') return;
            e.preventDefault();
            // dispatch() validates that ctx satisfies action.requires at runtime
            actions.dispatch(id, { layerId: app.activeLayerId ?? undefined });
        };
    }

    cleanup = tinykeys(window, bindings);
}

export function unregisterHotkeys() {
    cleanup?.();
    cleanup = null;
}
