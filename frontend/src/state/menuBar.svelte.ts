/**
 * Pin state for the application menu. Unpinned (default) → the menu lives in
 * the hamburger dropdown. Pinned → its contents move into a persistent top
 * bar and the hamburger hides. The choice survives reload.
 */
import { persistedState } from './persisted.svelte';

class MenuBarState {
    #pinned = persistedState('darkly.menuPinned', false);

    get pinned(): boolean {
        return this.#pinned.value;
    }

    toggle() {
        this.#pinned.value = !this.#pinned.value;
    }
}

export const menuBar = new MenuBarState();
