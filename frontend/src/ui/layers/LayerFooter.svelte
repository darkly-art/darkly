<script lang="ts">
    import { app } from '../../state/app.svelte';
    import NewLayerMenu from './NewLayerMenu.svelte';
    import VeilPickerModal from '../veils/VeilPickerModal.svelte';
    import VoidPickerModal from '../voids/VoidPickerModal.svelte';
    import FilterPickerModal from '../filters/FilterPickerModal.svelte';
    import { actions } from '../../actions/registry';
    import { tooltipForAction } from '../../config/store.svelte';
    import Icon from '../../icons/Icon.svelte';

    let { onupdate }: { onupdate: () => void } = $props();

    let menuOpen = $state(false);
    let pickerOpen = $state(false);
    let voidPickerOpen = $state(false);
    let filterPickerOpen = $state(false);

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


    // The new-layer / new-group footer buttons route through the action
    // registry so their tooltips can surface the bound hotkey (resolved
    // via `tooltipForAction`). The selection-aware "wrap or empty group"
    // logic lives on the action handler in actions/index.ts.
    function addNormalLayer() {
        actions.dispatch('newLayer');
        onupdate();
    }

    function addGroup() {
        actions.dispatch('newGroup');
        onupdate();
    }

    function pick(kind: 'layer' | 'group' | 'veil' | 'void' | 'filter') {
        menuOpen = false;
        if (kind === 'layer') addNormalLayer();
        else if (kind === 'group') addGroup();
        else if (kind === 'veil') pickerOpen = true;
        else if (kind === 'filter') filterPickerOpen = true;
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
        if (!app.engine || app.activeLayerId === null) return false;
        const layer = findNode(app.layerTree, app.activeLayerId);
        return Boolean(layer?.canHaveMask)
            && !hostHasMask(layer)
            && layer.editable !== false;
    });

    function addMask() {
        if (!canAddMask) return;
        actions.dispatch('addMask');
        onupdate();
    }

    let canDelete = $derived(
        app.activeVeilIndex !== null
            || (app.activeLayerId !== null && findNode(app.layerTree, app.activeLayerId) !== null),
    );

    let canDuplicate = $derived(
        app.activeLayerId !== null
            && findNode(app.layerTree, app.activeLayerId) !== null,
    );

    // Show the multi-selection count in the footer button tooltips so
    // the user has a heads-up that the trash/duplicate buttons will
    // operate on the whole selection, not just the active layer.
    let selectionSize = $derived(app.selectedLayerIds.size);
    let isMulti = $derived(selectionSize > 1);
    let deleteTooltip = $derived(
        isMulti
            ? `Delete (${selectionSize})`
            : tooltipForAction('Delete', 'deleteLayer'),
    );
    let duplicateTooltip = $derived(
        isMulti
            ? `Duplicate (${selectionSize})`
            : tooltipForAction('Duplicate', 'duplicateLayer'),
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
            <Icon name="fa6-solid:plus" />
        </button>
        <button
            class="footer-btn split-chevron new-layer-trigger"
            data-keep-open="new-layer"
            onclick={() => (menuOpen = !menuOpen)}
            title="New layer type…"
        >
            <Icon name="fa6-solid:chevron-down" />
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
        <Icon name="radix-icons:mask-on" />
    </button>

    <button
        class="footer-btn"
        onclick={duplicate}
        disabled={!canDuplicate}
        title={duplicateTooltip}
    >
        <Icon name="fa6-solid:clone" />
    </button>

    <button
        class="footer-btn danger"
        onclick={remove}
        disabled={!canDelete || (app.activeVeilIndex === null && !activeEditable)}
        title={deleteTooltip}
    >
        <Icon name="fa6-solid:trash" />
    </button>
</div>

{#if pickerOpen}
    <VeilPickerModal onclose={() => { pickerOpen = false; onupdate(); }} />
{/if}

{#if voidPickerOpen}
    <VoidPickerModal onclose={() => { voidPickerOpen = false; onupdate(); }} />
{/if}

{#if filterPickerOpen}
    <FilterPickerModal onclose={() => { filterPickerOpen = false; onupdate(); }} />
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
</style>
