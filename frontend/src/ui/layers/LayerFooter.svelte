<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { actions } from '../../actions/registry';
    import { tooltipForAction } from '../../config/store.svelte';
    import Icon from '../../icons/Icon.svelte';

    let { onupdate }: { onupdate: () => void } = $props();

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


    // The footer buttons route through the action registry so their tooltips
    // can surface the bound hotkey (resolved via `tooltipForAction`) and so
    // the behaviour — selection-aware "wrap or empty group", picker modals for
    // the typed kinds — has one home in actions/index.ts.
    function pick(actionId: string) {
        actions.dispatch(actionId);
        onupdate();
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
        app.activeLayerId !== null && findNode(app.layerTree, app.activeLayerId) !== null,
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
        // The `deleteLayer` action owns layer removal (including the error
        // toast and the tree refresh). The trash button just routes through it.
        actions.dispatch('deleteLayer');
        onupdate();
    }

    function duplicate() {
        actions.dispatch('duplicateLayer');
        onupdate();
    }
</script>

<div class="footer">
    <button
        class="footer-btn add-layer"
        onclick={() => pick('addLayer')}
        title={tooltipForAction('Add layer', 'addLayer')}
    >
        <Icon name="fa6-solid:plus" />
    </button>

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
        disabled={!canDelete || !activeEditable}
        title={deleteTooltip}
    >
        <Icon name="fa6-solid:trash" />
    </button>
</div>

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

    /* The add button keeps the split button's larger glyph, but not its
       trailing margin — that separated the welded plus+chevron unit from the
       rest, and a lone button just takes the footer's gap like its neighbours. */
    .footer > .footer-btn.add-layer {
        font-size: 18px;
    }
</style>
