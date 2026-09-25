<script lang="ts">
    /**
     * A list of layer-panel rows, at one depth.
     *
     * The dispatch it owns (divider, or an ordinary row) existed twice before:
     * once in `LayerPanel` for the root list and once in `LayerGroup` for a
     * container's children. Both are the same list at a different depth, so
     * both come here, and `LayerRow` recurses through this for its own
     * children.
     *
     * The divider is a tree node like any other and its slot in the list *is*
     * the viewport boundary, so it is a row rather than a special case in the
     * panel.
     */
    import type { LayerInfo } from '../../engine/protocol_gen';
    import type { RowNode } from './rowNode';
    import LayerRow from './LayerRow.svelte';
    import SpaceDivider from './SpaceDivider.svelte';

    let { nodes, depth = 0, onupdate }: {
        nodes: LayerInfo[];
        depth?: number;
        onupdate: () => void;
    } = $props();
</script>

{#each nodes as node, i (node.id)}
    {#if node.type === 'divider'}
        <SpaceDivider divider={node} empty={i === 0} {onupdate} />
    {:else}
        <LayerRow node={node as RowNode} {depth} {onupdate} />
    {/if}
{/each}
