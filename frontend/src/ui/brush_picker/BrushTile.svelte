<script lang="ts">
    import type { BrushInfo } from '../../state/brush_graph.svelte';
    import BrushPreviewStrip from './BrushPreviewStrip.svelte';

    interface Props {
        brush: BrushInfo;
        active?: boolean;
        onSelect: (brush: BrushInfo) => void;
    }
    let { brush, active = false, onSelect }: Props = $props();
</script>

<button
    class="brush-tile"
    class:active
    onclick={() => onSelect(brush)}
    title={brush.description || brush.name}
>
    <BrushPreviewStrip brushName={brush.name} />
    <span class="name">{brush.name}</span>
</button>

<style>
    /* Card-style container so each brush reads as one unit even when
     * the picker is dense. Stronger border + slightly inset bg gives
     * each tile clear visual edges against the picker surface. */
    .brush-tile {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        background: var(--bg);
        border: 1px solid var(--bg-active);
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        text-align: left;
        transition: background 0.1s, border-color 0.1s;
        /* Backstop for the grid `minmax(0, 1fr)` columns — children
         * (especially imgs) can't blow the tile out horizontally. */
        min-width: 0;
    }
    .brush-tile:hover {
        background: var(--bg-hover);
        border-color: var(--text-muted);
    }
    .brush-tile.active {
        border-color: var(--accent);
        background: color-mix(in srgb, var(--accent) 12%, var(--bg));
        box-shadow: 0 0 0 1px var(--accent) inset;
    }
    .name {
        font-size: 11px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
