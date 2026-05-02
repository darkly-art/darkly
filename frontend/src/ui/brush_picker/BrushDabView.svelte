<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { theme } from '../../state/theme.svelte';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        /** Display dimensions in CSS pixels. The PNG itself is rendered
         *  at a fixed engine-side size (matching the baked thumbnail
         *  path) and scaled by the browser to fit. */
        width?: number;
        height?: number;
    }
    let { width = 32, height = 32 }: Props = $props();

    /** Throttle window for rAF-driven refreshes — same cadence
     *  `BrushPreview.svelte` uses for its full-stroke editor preview. */
    const REFRESH_MS = 100;

    let dabUrl = $state('');
    /** Byte length that produced `dabUrl` — skips redundant Blob/URL
     *  churn when WASM hands back the same PNG (cache hit). */
    let lastLen = 0;

    /** Active polling budget. 30 frames ≈ 500ms at 60Hz — well past the
     *  ~10-30ms we measure for a single-dab render, so the pixels always
     *  arrive before we stop. */
    const POLL_FRAMES_PER_REQUEST = 30;
    let framesRemaining = 0;
    let rafHandle = 0;

    function refresh() {
        if (!app.handle) return;
        const bytes = app.handle.brush_active_dab_preview();
        if (!bytes || bytes.length === 0) return;
        if (bytes.length === lastLen && dabUrl) return;
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
        const next = URL.createObjectURL(blob);
        if (dabUrl) URL.revokeObjectURL(dabUrl);
        dabUrl = next;
        lastLen = bytes.length;
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

    // Reactive trigger: graph snapshot, active brush, or theme changes
    // invalidate the active-dab preview cache. Display size is CSS-only
    // and doesn't require a re-render.
    $effect(() => {
        void brushGraph.graph;
        void brushGraph.activeBrush;
        void theme.current;
        void app.handle;
        untrack(() => compressor.request());
    });

    onDestroy(() => {
        compressor.cancel();
        if (rafHandle) cancelAnimationFrame(rafHandle);
        if (dabUrl) URL.revokeObjectURL(dabUrl);
    });
</script>

<div class="brush-dab-view" style="--dab-w: {width}px; --dab-h: {height}px">
    {#if dabUrl}
        <img class="dab-img" src={dabUrl} alt="" />
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
        overflow: hidden;
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
