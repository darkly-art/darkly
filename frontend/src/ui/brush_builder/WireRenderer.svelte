<script lang="ts">
    interface Props {
        wirePaths: { path: string; color: string }[];
        dragWire: { path: string; color: string } | null;
        panX: number;
        panY: number;
        zoom: number;
    }

    let { wirePaths, dragWire, panX, panY, zoom }: Props = $props();
</script>

<!-- CSS transform applies pan/zoom. No will-change so the browser re-rasterizes
     at the target scale (crisp at all zoom levels, like React Flow).
     vector-effect="non-scaling-stroke" keeps stroke widths constant in screen px. -->
<svg
    class="wire-layer"
    style="transform: translate({panX}px, {panY}px) scale({zoom}); transform-origin: 0 0;"
>
    {#each wirePaths as wire}
        <path
            d={wire.path}
            stroke={wire.color}
            stroke-width="2"
            fill="none"
            opacity="0.8"
            vector-effect="non-scaling-stroke"
        />
    {/each}
    {#if dragWire}
        <path
            d={dragWire.path}
            stroke={dragWire.color}
            stroke-width="2"
            fill="none"
            opacity="0.5"
            stroke-dasharray="4"
            vector-effect="non-scaling-stroke"
        />
    {/if}
</svg>

<style>
    .wire-layer {
        position: absolute;
        inset: 0;
        pointer-events: none;
        overflow: visible;
    }
</style>
