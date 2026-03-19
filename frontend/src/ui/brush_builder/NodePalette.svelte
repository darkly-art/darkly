<script lang="ts">
    import { brushGraph, type NodeTypeInfo } from '../../state/brush_graph.svelte';

    interface Props {
        onaddnode: (typeId: string) => void;
    }

    let { onaddnode }: Props = $props();

    // Group node types by category.
    let categories = $derived((() => {
        const cats: Record<string, NodeTypeInfo[]> = {};
        for (const nt of brushGraph.nodeTypes) {
            const cat = nt.category || 'other';
            if (!cats[cat]) cats[cat] = [];
            cats[cat].push(nt);
        }
        return cats;
    })());

    let isOpen = $state(false);
</script>

<div class="palette-container">
    <button class="palette-toggle" onclick={() => isOpen = !isOpen}>
        + Add Node
    </button>

    {#if isOpen}
        <div class="palette-dropdown">
            {#each Object.entries(categories) as [category, types]}
                <div class="category-group">
                    <span class="category-label">{category}</span>
                    {#each types as nt}
                        <button
                            class="node-type-btn"
                            onclick={() => { onaddnode(nt.type_id); isOpen = false; }}
                            title={nt.type_id}
                        >
                            {nt.display_name}
                        </button>
                    {/each}
                </div>
            {/each}
        </div>
    {/if}
</div>

<style>
    .palette-container {
        position: relative;
    }
    .palette-toggle {
        background: #3a3a3a;
        border: 1px solid #555;
        border-radius: 4px;
        color: #ccc;
        cursor: pointer;
        font-size: 11px;
        padding: 4px 10px;
    }
    .palette-toggle:hover {
        background: #444;
    }
    .palette-dropdown {
        position: absolute;
        top: 100%;
        left: 0;
        margin-top: 4px;
        background: #2a2a2a;
        border: 1px solid #555;
        border-radius: 4px;
        padding: 4px;
        min-width: 140px;
        z-index: 100;
        box-shadow: 0 4px 12px rgba(0,0,0,0.5);
    }
    .category-group {
        margin-bottom: 4px;
    }
    .category-label {
        display: block;
        font-size: 9px;
        color: #888;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        padding: 2px 4px;
    }
    .node-type-btn {
        display: block;
        width: 100%;
        text-align: left;
        background: none;
        border: none;
        color: #ccc;
        cursor: pointer;
        font-size: 11px;
        padding: 3px 8px;
        border-radius: 3px;
    }
    .node-type-btn:hover {
        background: #3a3a3a;
    }
</style>
