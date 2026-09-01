<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';
    import LayerFooter from './LayerFooter.svelte';
    import SpaceDivider from './SpaceDivider.svelte';
    import { bindingSite } from '../../actions/binding_site';

    function refresh() {
        app.refreshLayerTree();
        app.requestFrame();
    }

    $effect(() => {
        if (app.engine) refresh();
    });

    function onDragOver(e: DragEvent) {
        e.preventDefault();
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
    }
</script>

<!-- The panel is the binding site for `layerPanel`-scoped hotkeys (e.g.
     Photoshop / GIMP `Delete`). `mouse: false` keeps individual layer
     thumbnails' own chord dispatch separate — only keyboard scope here. -->
<div class="panel" use:bindingSite={{
    name: 'layerPanel',
    ctx: () => ({ layerId: app.activeLayerId ?? undefined }),
    mouse: false,
}}>
    <div class="panel-header">
        <LayerFooter onupdate={refresh} />
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="layer-list" ondragover={onDragOver} ondrop={onDrop}>
        <SpaceDivider onupdate={refresh} />

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

<style>
    .panel {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-height: 0;
    }

    .panel-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding: 6px 12px 6px 8px;
        flex-shrink: 0;
    }

    .layer-list {
        position: relative;
        flex: 1;
        overflow-y: auto;
        min-height: 0;
    }

    .empty-message {
        padding: 16px;
        text-align: center;
        color: var(--text-dim);
        font-size: 12px;
    }
</style>
