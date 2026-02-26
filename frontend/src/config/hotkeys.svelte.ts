import { tinykeys } from 'tinykeys';
import { user } from './store.svelte';

let cleanup: (() => void) | null = null;

/**
 * Register all hotkeys from the resolved user config.
 * Call on init and whenever the preset changes.
 * `actions` maps HotkeyMap key names to handler functions.
 */
export function registerHotkeys(actions: Record<string, () => void>) {
    cleanup?.();

    const hotkeys = user.resolved.hotkeys;
    const bindings: Record<string, (e: KeyboardEvent) => void> = {};

    for (const [action, handler] of Object.entries(actions)) {
        const key = (hotkeys as any)[action];
        if (key && typeof key === 'string') {
            bindings[key] = (e: KeyboardEvent) => {
                const tag = (e.target as HTMLElement)?.tagName;
                if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
                e.preventDefault();
                handler();
            };
        }
    }

    cleanup = tinykeys(window, bindings);
}

export function unregisterHotkeys() {
    cleanup?.();
    cleanup = null;
}
