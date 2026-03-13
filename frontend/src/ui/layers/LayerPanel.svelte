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

<div class="layer-panel">
    <div class="panel-header">
        <span>Layers</span>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
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

    <div class="panel-actions">
        <button class="action-btn" onclick={addLayer} title="Add layer">+</button>
        <button class="action-btn" onclick={addGroup} title="Add group">&#x1F4C1;</button>
        <button class="action-btn" onclick={addMask} disabled={!canAddMask()} title="Add layer mask">&#x25D0;</button>
        <button class="action-btn" onclick={removeLayer} title="Delete layer">&#x1F5D1;</button>
    </div>
</div>

<style>
    .layer-panel {
        display: flex;
        flex-direction: column;
        height: 100%;
    }

    .panel-header {
        padding: 8px 12px;
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: #888;
        border-bottom: 1px solid #333;
    }

    .layer-list {
        flex: 1;
        overflow-y: auto;
        padding: 4px 0;
    }

    .empty-message {
        padding: 16px;
        text-align: center;
        color: #555;
        font-size: 12px;
    }

    .panel-actions {
        display: flex;
        gap: 4px;
        padding: 8px;
        border-top: 1px solid #333;
    }

    .action-btn {
        flex: 1;
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 4px;
        color: #ccc;
        cursor: pointer;
        padding: 4px;
        font-size: 14px;
    }

    .action-btn:hover:not(:disabled) {
        background: #333;
    }

    .action-btn:disabled {
        opacity: 0.35;
        cursor: default;
    }
</style>
