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
    <BrushPreviewStrip brushName={brush.name} icon={brush.icon} />
    <span class="name">{brush.name}</span>
</button>

<style>
    /* Raised well on the black picker slab — separation is fill contrast,
     * not a border. Hover/selected lighten the fill; no outlines, no
     * shadows. */
    .brush-tile {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        /* Buttons shrink-to-fit their content by default; fill the
         * (definite) grid track instead, so the preview strip — and
         * any percentage inside it — has a real width to resolve
         * against even when its content has no intrinsic size (the
         * icon fallback is just a 1em svg). */
        width: 100%;
        background: var(--bg-hover);
        border: none;
        border-radius: var(--radius-md);
        color: var(--text-muted);
        cursor: pointer;
        text-align: left;
        transition: background 0.1s, color 0.1s;
        /* Backstop for the grid `minmax(0, 1fr)` columns — children
         * (especially imgs) can't blow the tile out horizontally. */
        min-width: 0;
    }
    .brush-tile:hover {
        background: var(--bg-active);
        color: var(--text);
    }
    /* Loaded brush: a clearly lighter slab, not an outline. */
    .brush-tile.active {
        background: var(--thumb-bg);
        color: var(--text);
    }
    .name {
        font-size: 11px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
