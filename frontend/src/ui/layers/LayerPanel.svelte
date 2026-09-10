<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';
    import LayerFooter from './LayerFooter.svelte';
    import SpaceDivider from './SpaceDivider.svelte';
    import { bindingSite } from '../../actions/binding_site';
    import { layerDropTarget } from './dropTarget.svelte';

    function refresh() {
        app.refreshLayerTree();
        app.requestFrame();
    }

    $effect(() => {
        if (app.engine) refresh();
    });

</script>

<!-- The panel is the binding site for `layerPanel`-scoped hotkeys (e.g.
     Photoshop / GIMP `Delete`). `mouse: false` keeps individual layer
     thumbnails' own chord dispatch separate: only keyboard scope here. -->
<div class="panel" use:bindingSite={{
    name: 'layerPanel',
    ctx: () => ({ layerId: app.activeLayerId ?? undefined }),
    mouse: false,
}}>
    <div class="panel-header">
        <LayerFooter onupdate={refresh} />
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <!-- The list's own drop target is the empty space under the last row: a
         drop there means "below everything, at root", which is the one place
         the row gestures cannot reach when the bottom row is nested. Rows stop
         their own drag events, so this only ever sees the gap beneath them. -->
    <div
        class="layer-list"
        use:layerDropTarget={{ gap: app.dropRows.length, pin: 'min', onupdate: refresh }}
    >
        <!-- The divider is a tree node like any other; its slot in the list
             *is* the boundary, and dragging it is an ordinary layer move. -->
        {#each app.layerTree as node, i (node.id)}
            {#if node.type === 'divider'}
                <SpaceDivider divider={node} empty={i === 0} onupdate={refresh} />
            {:else if node.type === 'group'}
                <LayerGroup group={node} onupdate={refresh} />
            {:else}
                <LayerItem layer={node} onupdate={refresh} />
            {/if}
        {/each}

        <!-- The divider is always in the tree, so "no layers" means no rows
             besides it. -->
        {#if app.layerTree.filter((n) => n.type !== 'divider').length === 0}
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
