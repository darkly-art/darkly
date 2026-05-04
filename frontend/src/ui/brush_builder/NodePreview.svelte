<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { theme } from '../../state/theme.svelte';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        nodeId: number;
        width?: number;
        height?: number;
    }
    let { nodeId, width = 96, height = 96 }: Props = $props();

    /** Same throttle window as `BrushDabView` — debounces graph mutations
     *  during scrubbing so the engine sees ~10 render requests/sec at peak
     *  instead of one per Svelte tick. */
    const REFRESH_MS = 100;

    let imgUrl = $state('');
    /** Byte length that produced `imgUrl` — skip redundant Blob/URL churn
     *  when WASM hands back the same PNG (cache hit). */
    let lastLen = 0;

    /** Frame budget after each request — wide enough to cover the async
     *  readback (~10–30ms render + 1–2 frame poll latency). Once the bytes
     *  arrive and the cache fills, we stop polling until the next change. */
    const POLL_FRAMES_PER_REQUEST = 30;
    let framesRemaining = 0;
    let rafHandle = 0;

    function refresh() {
        if (!app.handle) return;
        const bytes = app.handle.brush_node_preview(nodeId);
        if (!bytes || bytes.length === 0) return;
        if (bytes.length === lastLen && imgUrl) return;
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
        const next = URL.createObjectURL(blob);
        if (imgUrl) URL.revokeObjectURL(imgUrl);
        imgUrl = next;
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
        // Kick the engine's render loop so `poll_pending` advances the
        // in-flight readback. Same trick `BrushDabView` uses.
        app.requestFrame();
        refresh();
        scheduleFrame();
    }

    // Reactive trigger: graph snapshot, active brush, or theme changes
    // invalidate the preview cache. Display size is CSS-only and doesn't
    // require a re-render.
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
        if (imgUrl) URL.revokeObjectURL(imgUrl);
    });
</script>

<div class="node-preview" style="--w: {width}px; --h: {height}px">
    {#if imgUrl}
        <img class="preview-img" src={imgUrl} alt="" />
    {:else}
        <div class="preview-placeholder"></div>
    {/if}
</div>

<style>
    .node-preview {
        width: var(--w);
        height: var(--h);
        margin: 4px auto 2px;
        border-radius: 3px;
        background: var(--bg);
        border: 1px solid color-mix(in srgb, var(--text) 8%, transparent);
        overflow: hidden;
        display: flex;
        align-items: center;
        justify-content: center;
    }
    .preview-img {
        width: 100%;
        height: 100%;
        display: block;
        image-rendering: auto;
    }
    .preview-placeholder {
        width: 100%;
        height: 100%;
    }
</style>
