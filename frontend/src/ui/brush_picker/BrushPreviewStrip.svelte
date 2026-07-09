<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { theme } from '../../state/theme.svelte';
    import { SignalCompressor } from '../../lib/signal_compressor';
    import BrushPreviewFallback from './BrushPreviewFallback.svelte';

    interface Props {
        /** Library brush name to look up in the engine's baked PNG cache. */
        brushName: string;
        /** Iconify icon rendered in place of the baked thumbnails — set
         *  for content-dependent brushes whose bake renders blank (see
         *  `BrushInfo.icon`). When present, the thumbnail fetches never
         *  fire, so no bake is triggered for the brush. */
        icon?: string | null;
    }
    let { brushName, icon = null }: Props = $props();

    /** Same throttle cadence as the dab and editor previews. */
    const REFRESH_MS = 100;

    /** Cached object URLs for the two PNGs we display. Object URLs are
     *  cheaper than data URLs across remounts (we hand the browser
     *  bytes once, not on every render), and they're trivially revoked
     *  when bytes change or the unit unmounts. */
    let strokeUrl = $state('');
    let dabUrl = $state('');
    /** Byte lengths that produced the current URLs — used to skip
     *  redundant Blob/URL churn on cache hits. */
    let lastStrokeLen = 0;
    let lastDabLen = 0;

    /** rAF poll budget — both bakes fit comfortably inside 30 frames. */
    const POLL_FRAMES_PER_REQUEST = 30;
    let framesRemaining = 0;
    let rafHandle = 0;

    function loadPng(
        bytes: Uint8Array | undefined,
        prevUrl: string,
        prevLen: number,
    ): { url: string; len: number } | null {
        if (!bytes || bytes.length === 0) return null;
        if (bytes.length === prevLen && prevUrl) return null;
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
        const next = URL.createObjectURL(blob);
        if (prevUrl) URL.revokeObjectURL(prevUrl);
        return { url: next, len: bytes.length };
    }

    async function refresh() {
        const engine = app.engine;
        if (!engine) return;
        const stroke = loadPng(
            (await engine.api.brushThumbnail({ name: brushName })).bytes,
            strokeUrl,
            lastStrokeLen,
        );
        if (stroke) {
            strokeUrl = stroke.url;
            lastStrokeLen = stroke.len;
        }
        const dab = loadPng(
            (await engine.api.brushDabThumbnail({ name: brushName })).bytes,
            dabUrl,
            lastDabLen,
        );
        if (dab) {
            dabUrl = dab.url;
            lastDabLen = dab.len;
        }
    }

    const compressor = new SignalCompressor(REFRESH_MS, () => {
        void refresh();
        framesRemaining = POLL_FRAMES_PER_REQUEST;
        scheduleFrame();
    });

    function scheduleFrame() {
        if (rafHandle) return;
        rafHandle = requestAnimationFrame(onFrame);
    }

    function onFrame() {
        rafHandle = 0;
        if (framesRemaining <= 0) return;
        framesRemaining--;
        app.requestFrame();
        void refresh();
        scheduleFrame();
    }

    // Reactive trigger: WASM handle becoming available, theme swaps,
    // and the brush name changing all require fresh thumbnails. The
    // icon fallback replaces the whole strip, so with an icon there is
    // nothing to fetch — and skipping the fetch skips the lazy bake.
    $effect(() => {
        void app.engine;
        void theme.current;
        void brushName;
        if (icon) return;
        untrack(() => compressor.request());
    });

    onDestroy(() => {
        compressor.cancel();
        if (rafHandle) cancelAnimationFrame(rafHandle);
        if (strokeUrl) URL.revokeObjectURL(strokeUrl);
        if (dabUrl) URL.revokeObjectURL(dabUrl);
    });
</script>

<!-- Dab + stroke read as a single image: shared rounded envelope, no
     internal gap or per-panel border. The row aspect is bound on the
     parent — square dab plus 320:120 stroke at equal height gives
     `(stroke_h + stroke_w) / stroke_h = 1 + 320/120 = 11/3`. -->
<div class="thumbs">
    {#if icon}
        <BrushPreviewFallback {icon} />
    {:else}
        <div class="dab">
            {#if dabUrl}
                <img src={dabUrl} alt="" />
            {/if}
        </div>
        <div class="stroke">
            {#if strokeUrl}
                <img src={strokeUrl} alt="" />
            {/if}
        </div>
    {/if}
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
