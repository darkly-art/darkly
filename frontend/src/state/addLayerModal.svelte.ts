/**
 * The add-layer modal's open state and which tab it opens on.
 *
 * `addLayer` opens it with no tab, landing on the first; `newFilterLayer`,
 * `newVeil` and `newVoid` deep-link to a named tab so the Layer menu and the
 * command palette keep offering each kind directly. `ui/layers/AddLayerModal.svelte`
 * mounts against this.
 */
class AddLayerModalState {
    open = $state(false);
    /** Tab title to land on, or `null` for the first tab. */
    tab = $state<string | null>(null);

    show(tab: string | null = null) {
        this.tab = tab;
        this.open = true;
    }

    hide() {
        this.open = false;
        this.tab = null;
    }
}

export const addLayerModal = new AddLayerModalState();
