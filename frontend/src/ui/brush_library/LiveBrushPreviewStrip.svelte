<!--
    Live equivalent of `BrushPreviewStrip`: same square-dab + S-curve
    layout, but the bytes come from the live engine (`brush_active_dab_preview`
    + `brush_stroke_preview`) instead of the library's baked PNG cache.

    Used wherever a preview of the *active* graph is needed: the brush
    builder's preview dock, and the picker dropdown's active strip when
    the artist has loaded a custom (unnamed) graph. Visually matches the
    preset preview so the dropdown looks consistent across both cases.
-->
<script lang="ts">
    import BrushDabView from './BrushDabView.svelte';
    import BrushStrokePreview from '../brush_builder/BrushStrokePreview.svelte';
    import BrushPreviewFallback from './BrushPreviewFallback.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';

    interface Props {
        /** Total width of the strip in CSS pixels. Height is derived from
         *  the 11:3 aspect ratio (square dab + S-curve at equal height). */
        width: number;
    }
    let { width }: Props = $props();

    // Strip aspect: dab is 1:1, stroke is 8:3, total 11:3.
    const stripHeight = $derived(Math.round((width * 3) / 11));
    const dabSize = $derived(stripHeight);
    const strokeWidth = $derived(width - stripHeight);
</script>

<div class="thumbs" style="width: {width}px; height: {stripHeight}px">
    <div class="dab" style="width: {dabSize}px; height: {dabSize}px">
        {#if brushGraph.previewIcon}
            <BrushPreviewFallback icon={brushGraph.previewIcon} />
        {:else}
            <BrushDabView width={dabSize} height={dabSize} />
        {/if}
    </div>
    <div class="stroke">
        <BrushStrokePreview width={strokeWidth} height={stripHeight} />
    </div>
</div>

<style>
    .thumbs {
        display: flex;
        background: var(--bg-hover);
        border-radius: 4px;
        overflow: hidden;
    }
    .dab {
        flex-shrink: 0;
    }
    .stroke {
        flex: 1;
        min-width: 0;
    }
</style>
