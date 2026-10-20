<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerFooter from './LayerFooter.svelte';
    import LayerRows from './LayerRows.svelte';
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

    <div class="layer-list">
        <LayerRows nodes={app.layerTree} onupdate={refresh} />

        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <!-- The tail: the space under the last row, filling the rest of the
             list. A drop here means "below everything, at root", which is the
             one place the row gestures cannot reach when the bottom row is
             nested. It is the gap below the last row, so its top edge is where
             that gap is and where its drop line draws. -->
        <div
            class="drop-tail"
            use:layerDropTarget={{ gap: app.dropRows.length, pin: 'min', onupdate: refresh }}
        >
            <!-- The divider is always in the tree, so "no layers" means no rows
                 besides it. -->
            {#if app.layerTree.filter((n) => n.type !== 'divider').length === 0}
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
        display: flex;
        flex-direction: column;
        flex: 1;
        overflow-y: auto;
        min-height: 0;
    }

    .drop-tail {
        flex: 1;
        min-height: 24px;
    }

    .empty-message {
        padding: 16px;
        text-align: center;
        color: var(--text-dim);
        font-size: 12px;
    }
</style>
