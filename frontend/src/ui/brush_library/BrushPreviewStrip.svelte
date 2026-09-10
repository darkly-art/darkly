<script lang="ts">
    import BrushThumb from './BrushThumb.svelte';

    interface Props {
        /** Library brush name to look up in the engine's baked PNG cache. */
        brushName: string;
        /** Glyph for the dab slot: set for content-dependent brushes, whose
         *  still-dab bake renders blank (see `BrushInfo.icon`). The stroke
         *  slot bakes normally either way. */
        icon?: string | null;
    }
    let { brushName, icon = null }: Props = $props();
</script>

<!-- Dab + stroke read as a single image: shared rounded envelope, no
     internal gap or per-panel border. The row aspect is bound here: square
     dab plus 8:3 stroke at equal height gives 1 + 8/3 = 11/3. -->
<div class="brush-thumbs strip">
    <div class="dab"><BrushThumb name={brushName} kind="dab" {icon} /></div>
    <div class="stroke"><BrushThumb name={brushName} /></div>
</div>

<style>
    .strip {
        width: 100%;
        aspect-ratio: 11 / 3;
    }
</style>
