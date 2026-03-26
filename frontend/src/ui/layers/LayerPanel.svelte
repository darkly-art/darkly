<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { toast } from '../../state/toast.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';

    function refresh() {
        app.refreshLayerTree();
        app.requestFrame();
    }

    // Refresh whenever handle becomes available
    $effect(() => {
        if (app.handle) {
            refresh();
        }
    });

    function addLayer() {
        if (app.handle) {
            const id = app.handle.add_raster_layer();
            app.activeLayerId = id;
            refresh();
        }
    }

    function addGroup() {
        if (app.handle) {
            const id = app.handle.add_group();
            app.activeLayerId = id;
            refresh();
        }
    }

    function removeLayer() {
        if (app.handle && app.activeLayerId !== null) {
            try {
                app.handle.remove_layer(app.activeLayerId);
                app.activeLayerId = null;
                refresh();
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        }
    }

    function addMask() {
        if (!app.handle || app.activeLayerId === null) return;
        // Find the active layer info to check if it's raster and doesn't already have a mask
        const layer = findLayer(app.layerTree, app.activeLayerId);
        if (!layer || layer.hasMask) return;
        if (layer.type !== 'raster' && layer.type !== 'group') return;
        app.handle.add_mask(app.activeLayerId);
        refresh();
    }

    function findLayer(nodes: any[], id: number): any | null {
        for (const n of nodes) {
            if (n.id === id) return n;
            if (n.children) {
                const found = findLayer(n.children, id);
                if (found) return found;
            }
        }
        return null;
    }

    let canAddMask = $derived(() => {
        if (!app.handle || app.activeLayerId === null) return false;
        const layer = findLayer(app.layerTree, app.activeLayerId);
        return (layer?.type === 'raster' || layer?.type === 'group') && !layer.hasMask;
    });

    function onDragOver(e: DragEvent) {
        e.preventDefault();
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
        // Handled by individual items/groups
    }
</script>

<div class="panel expanded">
    <div class="panel-header">
        <span class="panel-title">Layers</span>
        <div class="panel-actions">
            <button class="panel-btn" onclick={addLayer} title="Add Layer"><i class="fa-solid fa-plus"></i></button>
            <button class="panel-btn" onclick={addGroup} title="Add Group"><i class="fa-solid fa-folder-plus"></i></button>
            <button class="panel-btn" onclick={addMask} title="Add Mask"><i class="fa-solid fa-mask"></i></button>
            <button class="panel-btn danger" onclick={removeLayer} title="Delete Layer"><i class="fa-solid fa-trash"></i></button>
        </div>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="panel-body">
        <div class="layer-list" ondragover={onDragOver} ondrop={onDrop}>
            {#each app.layerTree as node (node.id)}
                {#if node.type === 'group'}
                    <LayerGroup group={node} onupdate={refresh} />
                {:else}
                    <LayerItem layer={node} onupdate={refresh} />
                {/if}
            {/each}

            {#if app.layerTree.length === 0}
                <div class="empty-message">No layers</div>
            {/if}
        </div>
    </div>
</div>

<style>
    .panel {
        display: flex;
        flex-direction: column;
        flex: 1;
    }

    .panel-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 10px 12px;
        background: var(--bg-hover);
    }

    .panel-title {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text);
    }

    .panel-actions {
        display: flex;
        gap: 2px;
    }

    .panel-btn {
        width: 26px;
        height: 26px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 5px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
        transition: background 0.1s, color 0.1s;
    }

    .panel-btn:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .panel-btn.danger:hover {
        color: var(--danger);
    }

    .panel-body {
        display: flex;
        flex-direction: column;
        flex: 1;
    }

    .layer-list {
        flex: 1;
        overflow-y: auto;
    }

    .empty-message {
        padding: 16px;
        text-align: center;
        color: var(--text-dim);
        font-size: 12px;
    }
</style>
