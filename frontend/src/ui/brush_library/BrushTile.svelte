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
    /* Tiles are the theme's own neutrals. The pack's identity is already under
     * them (the section they sit on is filled with its surface), and the grid
     * is the largest area in the picker, so it is the last place that should be
     * spending colour. The one vivid mark is the outline on the loaded brush.
     *
     * Being the theme's means they take the theme's text too, which is what
     * keeps a tile legible whatever a pack is made of and in either theme. */
    .brush-tile {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        /* Buttons shrink-to-fit their content by default; fill the
         * (definite) grid track instead, so the preview strip (and
         * any percentage inside it) has a real width to resolve
         * against even when its content has no intrinsic size (the
         * icon fallback is just a 1em svg). */
        width: 100%;
        background: var(--bg-hover);
        border: none;
        border-radius: var(--radius-md);
        color: var(--text-muted);
        cursor: pointer;
        text-align: left;
        transition: background 0.1s, outline-color 0.1s;
        /* Backstop for the grid `minmax(0, 1fr)` columns: children
         * (especially imgs) can't blow the tile out horizontally. */
        min-width: 0;
    }
    .brush-tile:hover {
        background: var(--bg-active);
        color: var(--text);
    }
    /* The loaded brush: the lightest slab of the three, ringed in one pixel of
     * the pack's chroma: a strand, the same as every other place chroma is
     * spent, and all it takes to be findable in a grid. */
    .brush-tile.active {
        background: var(--thumb-bg);
        color: var(--text);
        outline: 1px solid var(--pack-chroma);
        outline-offset: -1px;
    }
    .name {
        font-size: 11px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
