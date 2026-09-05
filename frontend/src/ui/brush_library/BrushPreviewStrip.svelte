<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { theme } from '../../state/theme.svelte';
    import { BakedThumbnail } from '../../lib/bakedThumbnail.svelte';
    import BrushPreviewFallback from './BrushPreviewFallback.svelte';

    interface Props {
        /** Library brush name to look up in the engine's baked PNG cache. */
        brushName: string;
        /** Iconify icon rendered in the dab slot in place of a baked dab
         *  thumbnail: set for content-dependent brushes, whose still-dab
         *  bake renders blank because one stationary sample has no motion
         *  for a displacement to reveal (see `BrushInfo.icon`). When
         *  present the dab fetch never fires, so no dab bake is triggered.
         *  The stroke slot always fetches: those brushes' stroke previews
         *  are staged over a field they can transport. */
        icon?: string | null;
    }
    let { brushName, icon = null }: Props = $props();

    const stroke = new BakedThumbnail(async () =>
        app.engine ? (await app.engine.api.brushThumbnail({ name: brushName })).bytes : undefined);
    const dab = new BakedThumbnail(async () =>
        app.engine && !icon
            ? (await app.engine.api.brushDabThumbnail({ name: brushName })).bytes
            : undefined);

    // Reactive trigger: WASM handle becoming available, theme swaps,
    // and the brush name changing all require fresh thumbnails. The icon
    // only replaces the dab half, so the stroke is always worth fetching;
    // the dab fetcher is what skips its bake when an icon occupies its slot.
    $effect(() => {
        void app.engine;
        void theme.current;
        void brushName;
        void icon;
        untrack(() => {
            stroke.request();
            dab.request();
        });
    });

    onDestroy(() => {
        stroke.destroy();
        dab.destroy();
    });
</script>

<!-- Dab + stroke read as a single image: shared rounded envelope, no
     internal gap or per-panel border. The row aspect is bound on the
     parent: square dab plus 320:120 stroke at equal height gives
     `(stroke_h + stroke_w) / stroke_h = 1 + 320/120 = 11/3`. -->
<div class="thumbs">
    <div class="dab">
        {#if icon}
            <BrushPreviewFallback {icon} />
        {:else if dab.url}
            <img src={dab.url} alt="" />
        {/if}
    </div>
    <div class="stroke">
        {#if stroke.url}
            <img src={stroke.url} alt="" />
        {/if}
    </div>
</div>

<style>
    .thumbs {
        width: 100%;
        aspect-ratio: 11 / 3;
        display: flex;
        background: var(--bg-hover);
        border-radius: 4px;
        overflow: hidden;
    }
    .dab {
        aspect-ratio: 1;
        height: 100%;
        flex-shrink: 0;
        overflow: hidden;
    }
    .stroke {
        flex: 1;
        height: 100%;
        overflow: hidden;
    }
    .dab img,
    .stroke img {
        width: 100%;
        height: 100%;
        display: block;
        image-rendering: auto;
    }
</style>
