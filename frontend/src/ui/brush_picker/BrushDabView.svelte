<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { theme } from '../../state/theme.svelte';
    import { rgbaToDataUrl } from '../layers/thumbnails';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        /** Target render / display dimensions, in CSS pixels. */
        width?: number;
        height?: number;
    }
    let { width = 32, height = 32 }: Props = $props();

    /** Throttle window for rAF-driven refreshes — same cadence
     *  `BrushPreview.svelte` uses for its full-stroke editor preview. */
    const REFRESH_MS = 100;

    let dataUrl = $state('');
    let imgW = $state(0);
    let imgH = $state(0);
    /** Cheap hash to skip redundant data-URL encodes when WASM hands
     *  back the same pixels (cache hit). */
    let lastHash = 0;

    function hashRgba(bytes: Uint8Array): number {
        let h = 2166136261 >>> 0;
        const step = Math.max(1, Math.floor(bytes.length / 256));
        for (let i = 0; i < bytes.length; i += step) {
            h = ((h ^ bytes[i]) * 16777619) >>> 0;
        }
        return h;
    }

    /** Active polling budget. 30 frames ≈ 500ms at 60Hz — well past the
     *  ~10-30ms we measure for a single-dab render, so the pixels always
     *  arrive before we stop. */
    const POLL_FRAMES_PER_REQUEST = 30;
    let framesRemaining = 0;
    let rafHandle = 0;

    function refresh() {
        if (!app.handle) return;
        const w = width;
        const h = height;
        const rgba = app.handle.brush_active_dab_preview(w, h);
        if (!rgba || rgba.length === 0) return;
        if (rgba.length !== w * h * 4) return;
        const hh = hashRgba(rgba);
        if (hh !== lastHash || dataUrl === '' || imgW !== w || imgH !== h) {
            lastHash = hh;
            imgW = w;
            imgH = h;
            dataUrl = rgbaToDataUrl(rgba, w, h);
        }
    }

    const compressor = new SignalCompressor(REFRESH_MS, () => {
        refresh();
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
        // Same trick as BrushPreview — kick the engine's render loop so
        // `poll_pending` advances the in-flight readback.
        app.requestFrame();
        refresh();
        scheduleFrame();
    }

    // Reactive trigger: graph snapshot, active brush, theme, or size
    // changes all invalidate the active-dab preview cache.
    $effect(() => {
        void brushGraph.graph;
        void brushGraph.activeBrush;
        void theme.current;
        void app.handle;
        void width;
        void height;
        untrack(() => compressor.request());
    });

    onDestroy(() => {
        compressor.cancel();
        if (rafHandle) cancelAnimationFrame(rafHandle);
    });
</script>

<div class="brush-dab-view" style="--dab-w: {width}px; --dab-h: {height}px">
    {#if dataUrl}
        <img class="dab-img" src={dataUrl} alt="" width={imgW} height={imgH} />
    {:else}
        <div class="dab-placeholder"></div>
    {/if}
</div>

<style>
    .brush-dab-view {
        position: relative;
        width: var(--dab-w);
        height: var(--dab-h);
        flex-shrink: 0;
    }
    .dab-img {
        width: 100%;
        height: 100%;
        display: block;
        image-rendering: auto;
    }
    .dab-placeholder {
        width: 100%;
        height: 100%;
    }
</style>
