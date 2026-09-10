<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerProperties from './LayerProperties.svelte';
    import GroupProperties from './GroupProperties.svelte';
    import TextProperties from './TextProperties.svelte';
    import VoidProperties from '../voids/VoidProperties.svelte';
    import FilterProperties from '../filters/FilterProperties.svelte';

    let activeLayer = $derived(app.activeNode);

</script>

<div class="panel">
    <div class="panel-body">
        {#if activeLayer}
            {#if activeLayer.type !== 'filter'}
                <LayerProperties node={activeLayer} />
            {/if}
            {#if activeLayer.type === 'group'}
                <GroupProperties group={activeLayer} />
            {:else if activeLayer.type === 'void'}
                <VoidProperties node={activeLayer} />
            {:else if activeLayer.type === 'filter'}
                <FilterProperties node={activeLayer} />
            {:else if activeLayer.type === 'vector'}
                <TextProperties node={activeLayer} />
            {/if}
        {:else}
            <div class="empty">No selection</div>
        {/if}
    </div>
</div>

<style>
    /* Fills the docking group's body: the tab bar supplies the title and the
       group frame supplies the border, so this panel no longer imposes the
       sidebar-stack chrome (border-top / max-height:50%) it once did. */
    .panel {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-height: 0;
        overflow-y: auto;
    }

    .panel-body {
        padding: 8px 12px;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .empty {
        font-size: 12px;
        color: var(--text-dim);
        text-align: center;
        padding: 8px 0;
    }
</style>
