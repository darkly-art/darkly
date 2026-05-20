<script lang="ts">
    import { app } from '../../state/app.svelte';
    import NewLayerMenu from './NewLayerMenu.svelte';
    import VeilPickerModal from '../veils/VeilPickerModal.svelte';
    import VoidPickerModal from '../voids/VoidPickerModal.svelte';
    import { actions } from '../../actions/registry';

    let { onupdate }: { onupdate: () => void } = $props();

    let menuOpen = $state(false);
    let pickerOpen = $state(false);
    let voidPickerOpen = $state(false);

    function findNode(nodes: any[], id: number): any | null {
        for (const n of nodes) {
            if (n.id === id) return n;
            if (n.children) {
                const found = findNode(n.children, id);
                if (found) return found;
            }
        }
        return null;
    }


    function addNormalLayer() {
        if (!app.handle) return;
        const id = app.handle.add_raster_layer(app.activeLayerId ?? -1);
        app.selectLayer(id);
        onupdate();
    }

    function addGroup() {
        if (!app.handle) return;
        const id = app.handle.add_group(app.activeLayerId ?? -1);
        app.selectLayer(id);
        onupdate();
    }

    function pick(kind: 'layer' | 'group' | 'veil' | 'void') {
        menuOpen = false;
        if (kind === 'layer') addNormalLayer();
        else if (kind === 'group') addGroup();
        else if (kind === 'veil') pickerOpen = true;
        else voidPickerOpen = true;
    }

    function hostHasMask(layer: any): boolean {
        return Array.isArray(layer?.modifiers)
            && layer.modifiers.some((m: any) => m.kind === 'mask');
    }

    // Effective editability of the active layer — mirrors the engine's
    // `is_node_editable` (locked node OR any ancestor locked → not editable).
    // Used to grey out destructive footer actions so users don't get the
    // "drag the slider, nothing happens" feedback loop.
    let activeEditable = $derived.by(() => {
        if (app.activeLayerId === null) return true;
        const layer = findNode(app.layerTree, app.activeLayerId);
        return layer ? layer.editable !== false : true;
    });

    let canAddMask = $derived.by(() => {
        if (!app.handle || app.activeLayerId === null) return false;
        const layer = findNode(app.layerTree, app.activeLayerId);
        return (layer?.type === 'raster' || layer?.type === 'group' || layer?.type === 'void')
            && !hostHasMask(layer)
            && layer.editable !== false;
    });

    function addMask() {
        if (!app.handle || app.activeLayerId === null) return;
        if (!canAddMask) return;
        const hostId = app.activeLayerId;
        app.handle.add_mask(hostId);
        // After add_mask the host gains a mask modifier; refresh tree, then
        // activate the modifier id (the new paint target) so strokes land
        // on the mask without a session redirect.
        onupdate();
        const layer = findNode(app.layerTree, hostId);
        const mask = layer?.modifiers?.find((m: any) => m.kind === 'mask');
        if (mask) app.selectLayer(mask.id);
    }

    let canDelete = $derived(
        app.activeVeilIndex !== null
            || (app.activeLayerId !== null && findNode(app.layerTree, app.activeLayerId) !== null),
    );

    let canDuplicate = $derived(
        app.activeLayerId !== null
            && findNode(app.layerTree, app.activeLayerId) !== null,
    );

    function remove() {
        // The `deleteLayer` action handles both veil-remove and layer-
        // remove (including toast on error and tree refresh). The trash
        // button just routes through it.
        actions.dispatch('deleteLayer');
        onupdate();
    }

    function duplicate() {
        actions.dispatch('duplicateLayer');
        onupdate();
    }
</script>

<div class="footer">
    <div class="split-btn">
        <button
            class="footer-btn split-main"
            onclick={addNormalLayer}
            title="New layer"
        >
            <i class="fa-solid fa-plus"></i>
        </button>
        <button
            class="footer-btn split-chevron new-layer-trigger"
            onclick={() => (menuOpen = !menuOpen)}
            title="New layer type…"
        >
            <i class="fa-solid fa-chevron-down"></i>
        </button>
        {#if menuOpen}
            <NewLayerMenu onpick={pick} onclose={() => (menuOpen = false)} />
        {/if}
    </div>

    <button
        class="footer-btn"
        onclick={addMask}
        disabled={!canAddMask}
        title="Add mask"
    >
        <svg class="mask-glyph" viewBox="0 0 16 10" aria-hidden="true">
            <rect width="16" height="10" rx="1" fill="currentColor" />
            <circle cx="8" cy="5" r="3" fill="var(--bg)" />
        </svg>
    </button>

    <button
        class="footer-btn"
        onclick={duplicate}
        disabled={!canDuplicate}
        title="Duplicate"
    >
        <i class="fa-solid fa-clone"></i>
    </button>

    <button
        class="footer-btn danger"
        onclick={remove}
        disabled={!canDelete || (app.activeVeilIndex === null && !activeEditable)}
        title="Delete"
    >
        <i class="fa-solid fa-trash"></i>
    </button>
</div>

{#if pickerOpen}
    <VeilPickerModal onclose={() => { pickerOpen = false; onupdate(); }} />
{/if}

{#if voidPickerOpen}
    <VoidPickerModal onclose={() => { voidPickerOpen = false; onupdate(); }} />
{/if}

<style>
    .footer {
        display: flex;
        align-items: center;
        gap: 2px;
    }

    .footer-btn {
        width: 26px;
        height: 26px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: var(--bg);
        border: none;
        border-radius: var(--radius-sm);
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
        transition: background var(--transition-fast), color var(--transition-fast);
    }

    .footer > .footer-btn {
        width: 34px;
        height: 34px;
        font-size: 14px;
    }

    .footer-btn:hover:not(:disabled) {
        background: var(--bg-hover);
        color: var(--text);
    }

    .footer-btn.danger:hover:not(:disabled) {
        color: var(--danger);
    }

    .footer-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .split-btn {
        position: relative;
        display: flex;
        align-items: center;
        margin-right: 4px;
    }

    .split-main {
        width: 34px;
        height: 34px;
        border-top-right-radius: 0;
        border-bottom-right-radius: 0;
        padding-right: 0;
        font-size: 18px;
    }

    .split-chevron {
        width: 16px;
        height: 34px;
        font-size: 9px;
        border-top-left-radius: 0;
        border-bottom-left-radius: 0;
        padding-left: 0;
        border-left: 1px solid var(--bg);
    }

    .split-main + .split-chevron {
        margin-left: 0;
    }

    .mask-glyph {
        width: 18px;
        height: 11px;
        display: block;
    }
</style>
