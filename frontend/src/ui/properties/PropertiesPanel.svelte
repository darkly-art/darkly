<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerProperties from './LayerProperties.svelte';
    import GroupProperties from './GroupProperties.svelte';
    import VeilProperties from '../veils/VeilProperties.svelte';
    import VoidProperties from '../voids/VoidProperties.svelte';

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

    let activeLayer = $derived(
        app.activeLayerId !== null ? findNode(app.layerTree, app.activeLayerId) : null,
    );

    // `activeVeilIndex` is a chain position (the engine's `index` field on
    // each VeilInfo), not a position in `veilList` — the list is returned in
    // reverse chain order for display. Look up by `index` so the two stay
    // aligned regardless of length.
    let activeVeil = $derived(
        app.activeVeilIndex !== null
            ? app.veilList.find((v: { index: number }) => v.index === app.activeVeilIndex) ?? null
            : null,
    );
</script>

<div class="panel">
    <div class="panel-body">
        {#if activeVeil}
            <VeilProperties veil={activeVeil} />
        {:else if activeLayer}
            <LayerProperties node={activeLayer} />
            {#if activeLayer.type === 'group'}
                <GroupProperties group={activeLayer} />
            {:else if activeLayer.type === 'void'}
                <VoidProperties node={activeLayer} />
            {/if}
        {:else}
            <div class="empty">No selection</div>
        {/if}
    </div>
</div>

<style>
    .panel {
        display: flex;
        flex-direction: column;
        border-top: 1px solid var(--bg-hover);
        max-height: 50%;
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
