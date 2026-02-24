<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerItem from './LayerItem.svelte';

    let layerTree = $state<any[]>([]);

    export function refreshTree() {
        if (app.handle) {
            const tree = app.handle.layer_tree();
            layerTree = Array.isArray(tree) ? tree : [];
        }
    }

    // Refresh whenever handle becomes available
    $effect(() => {
        if (app.handle) {
            refreshTree();
        }
    });

    function addLayer() {
        if (app.handle) {
            const id = app.handle.add_raster_layer();
            app.activeLayerId = Number(id);
            refreshTree();
        }
    }

    function removeLayer() {
        if (app.handle && app.activeLayerId !== null) {
            app.handle.remove_layer(BigInt(app.activeLayerId));
            app.activeLayerId = null;
            refreshTree();
        }
    }

    function onDrop(e: DragEvent, targetId: number, position: 'before' | 'after') {
        e.preventDefault();
        const draggedId = e.dataTransfer?.getData('text/plain');
        if (!draggedId || !app.handle) return;

        // For now, simple reorder within the flat layer list
        // Full tree reorder requires the move_layer WASM API
        refreshTree();
    }
</script>

<div class="layer-panel">
    <div class="panel-header">
        <span>Layers</span>
    </div>

    <div class="layer-list">
        {#each layerTree as layer (layer.id)}
            <LayerItem {layer} onupdate={refreshTree} />
        {/each}

        {#if layerTree.length === 0}
            <div class="empty-message">No layers</div>
        {/if}
    </div>

    <div class="panel-actions">
        <button class="action-btn" onclick={addLayer} title="Add layer">+</button>
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

    .action-btn:hover {
        background: #333;
    }
</style>
