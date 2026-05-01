<script lang="ts">
    import type { XmlNode } from '../../state/brush_inspector.svelte';
    import Self from './XmlNodeView.svelte';

    interface Props {
        node: XmlNode;
        depth?: number;
    }
    let { node, depth = 0 }: Props = $props();
</script>

<div class="node" style:padding-left={`${depth * 12}px`}>
    <div class="row">
        <span class="tag">&lt;{node.tag}&gt;</span>
        {#each node.attrs as [k, v] (k)}
            <span class="attr"
                ><span class="key">{k}</span>=<span class="val">{v}</span></span
            >
        {/each}
    </div>
    {#if node.text}
        <div class="text" style:padding-left={`${(depth + 1) * 12}px`}>
            <code>{node.text}</code>
        </div>
    {/if}
    {#each node.children as child, i (i)}
        <Self node={child} depth={depth + 1} />
    {/each}
</div>

<style>
    .node {
        font-family: monospace;
        font-size: 0.85rem;
        line-height: 1.5;
    }
    .row {
        display: flex;
        flex-wrap: wrap;
        gap: 8px 12px;
        align-items: baseline;
    }
    .tag {
        color: var(--accent);
        font-weight: 600;
    }
    .attr {
        color: var(--text);
    }
    .key {
        color: var(--text-muted);
    }
    .val {
        color: var(--text);
        background: var(--bg-hover);
        padding: 1px 5px;
        border-radius: 3px;
    }
    .text {
        color: var(--text-muted);
        font-style: italic;
    }
    .text code {
        background: var(--bg);
        padding: 1px 5px;
        border-radius: 3px;
        font-style: normal;
        color: var(--text);
    }
</style>
