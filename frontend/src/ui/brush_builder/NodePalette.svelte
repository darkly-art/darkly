<script lang="ts">
    import { brushGraph, type NodeTypeInfo } from '../../state/brush_graph.svelte';

    interface Props {
        onaddnode: (typeId: string) => void;
    }

    let { onaddnode }: Props = $props();

    // Group node types by category. `internal` is a hidden category used by
    // the engine for synthesised terminals (e.g. `preview_terminal` for the
    // per-node preview pipeline) — these are not user-placeable and would
    // confuse the palette UI.
    let categories = $derived((() => {
        const cats: Record<string, NodeTypeInfo[]> = {};
        for (const nt of brushGraph.nodeTypes) {
            if (nt.category === 'internal') continue;
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
        background: var(--bg-hover);
        border: none;
        border-radius: 4px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 11px;
        padding: 4px 10px;
        transition: background 0.1s, color 0.1s;
    }
    .palette-toggle:hover {
        background: var(--bg-active);
        color: var(--text);
    }
    .palette-dropdown {
        position: absolute;
        bottom: 100%;
        left: 0;
        margin-bottom: 4px;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
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
        color: var(--text-muted);
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
        color: var(--text);
        cursor: pointer;
        font-size: 11px;
        padding: 3px 8px;
        border-radius: 3px;
        transition: background 0.1s;
    }
    .node-type-btn:hover {
        background: var(--bg-hover);
    }
</style>
