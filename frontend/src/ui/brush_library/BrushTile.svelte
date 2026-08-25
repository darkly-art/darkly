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
    /* A tile wears its group's colours, inherited as custom properties from
     * whatever renders it rather than passed as props: nothing about a brush
     * changes with the pack it is being shown under, so the colour is context,
     * not data. The fallbacks are the theme's neutrals, which is what a group
     * with no pack behind it supplies anyway.
     *
     * A tile is a *tint*, not the pack's colour at full strength — the pack
     * card and the section header carry that, and a grid of saturated slabs
     * would drown the previews they exist to show. Each state mixes the pack's
     * surface into the neutral one it would otherwise have had, so a group with
     * no pack lands exactly on the old greys and separation stays fill
     * contrast rather than a border. */
    .brush-tile {
        --primary: var(--pack-primary, var(--bg-hover));
        --secondary: var(--pack-secondary, var(--text-muted));
        --tint: 24%;
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
        background: color-mix(in srgb, var(--primary) var(--tint), var(--bg-hover));
        border: none;
        border-radius: var(--radius-md);
        color: color-mix(in srgb, var(--secondary) 40%, var(--text-muted));
        cursor: pointer;
        text-align: left;
        transition: background 0.1s, outline-color 0.1s;
        /* Backstop for the grid `minmax(0, 1fr)` columns — children
         * (especially imgs) can't blow the tile out horizontally. */
        min-width: 0;
    }
    .brush-tile:hover {
        background: color-mix(in srgb, var(--primary) var(--tint), var(--bg-active));
        color: color-mix(in srgb, var(--secondary) 40%, var(--text));
    }
    /* The loaded brush: the lightest slab of the three, ringed in the pack's
     * ink so it stays findable once several tiles are tinted alike. */
    .brush-tile.active {
        background: color-mix(in srgb, var(--primary) var(--tint), var(--thumb-bg));
        color: color-mix(in srgb, var(--secondary) 40%, var(--text));
        outline: 1px solid color-mix(in srgb, var(--secondary) 55%, transparent);
        outline-offset: -1px;
    }
    .name {
        font-size: 11px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
