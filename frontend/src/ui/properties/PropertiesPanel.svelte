<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { textSession } from '../../tools/text.svelte';
    import LayerProperties from './LayerProperties.svelte';
    import GroupProperties from './GroupProperties.svelte';
    import TextProperties from './TextProperties.svelte';
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
        {:else if activeLayer || textSession.placement}
            <!-- Filter layers honor neither opacity nor blend mode yet (the
                 first slice composites at full strength), so the blend/opacity
                 controls are hidden rather than shown as inert. Re-enable when
                 opacity/blend honoring lands. -->
            {#if activeLayer && activeLayer.type !== 'filter'}
                <LayerProperties node={activeLayer} />
            {/if}
            {#if activeLayer?.type === 'group'}
                <GroupProperties group={activeLayer} />
            {:else if activeLayer?.type === 'void'}
                <VoidProperties node={activeLayer} />
            {/if}
            <!-- One TextProperties instance, rendered from a single template
                 position so Svelte keeps it across the pending→bound transition
                 (a fresh placement becoming a real layer). A second usage would
                 remount and drop the caret on the first keystroke. -->
            {#if activeLayer?.type === 'vector' || textSession.placement}
                <TextProperties node={activeLayer?.type === 'vector' ? activeLayer : null} />
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
