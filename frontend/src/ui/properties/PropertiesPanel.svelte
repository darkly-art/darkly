<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerProperties from './LayerProperties.svelte';
    import GroupProperties from './GroupProperties.svelte';
    import VeilProperties from '../veils/VeilProperties.svelte';

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

    let title = $derived(
        activeVeil ? `Veil — ${activeVeil.type}`
            : activeLayer ? activeLayer.name ?? 'Properties'
            : 'Properties',
    );
</script>

<div class="panel">
    <!-- <div class="panel-header">
        <span class="panel-title">{title} Properties</span>
    </div> -->
    <div class="panel-body">
        {#if activeVeil}
            <VeilProperties veil={activeVeil} />
        {:else if activeLayer}
            <LayerProperties node={activeLayer} />
            {#if activeLayer.type === 'group'}
                <GroupProperties group={activeLayer} />
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
        flex-shrink: 0;
    }

    .panel-header {
        padding: 10px 12px;
        background: var(--bg-hover);
    }

    .panel-title {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        display: block;
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
