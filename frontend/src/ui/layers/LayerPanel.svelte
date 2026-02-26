<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';

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

    function addGroup() {
        if (app.handle) {
            const id = app.handle.add_group();
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
        {#each layerTree as node (node.id)}
            {#if node.type === 'group'}
                <LayerGroup group={node} onupdate={refreshTree} />
            {:else}
                <LayerItem layer={node} onupdate={refreshTree} />
            {/if}
        {/each}

        {#if layerTree.length === 0}
            <div class="empty-message">No layers</div>
        {/if}
    </div>

    <div class="panel-actions">
        <button class="action-btn" onclick={addLayer} title="Add layer">+</button>
        <button class="action-btn" onclick={addGroup} title="Add group">&#x1F4C1;</button>
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
