<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { theme } from '../../state/theme.svelte';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        /** Display dimensions, in CSS pixels. The PNG itself is rendered
         *  at a fixed engine-side size (`BRUSH_THUMBNAIL_SIZE`) and scaled
         *  by the browser to fit — same shape as `BrushDabView`. */
        width: number;
        height: number;
    }
    let { width, height }: Props = $props();

    /** Throttle window — matches Krita's 100ms FIRST_ACTIVE compressor for
     *  its live brush preview (kis_preset_live_preview_view.cpp:24). */
    const REFRESH_MS = 100;

    let dataUrl = $state('');
    /** Byte length that produced `dataUrl` — skips redundant Blob/URL
     *  churn when WASM hands back the same PNG (cache hit). */
    let lastLen = 0;

    /**
     * Budget of animation frames during which we actively poll WASM for
     * the async readback result. 30 frames ≈ 500ms on a 60Hz display —
     * far longer than a typical editor-preview render (~10-30ms) so the
     * pixels always arrive before we stop.
     */
    const POLL_FRAMES_PER_REQUEST = 30;
    let framesRemaining = 0;
    let rafHandle = 0;

    function refresh() {
        if (!app.handle) return;
        const bytes = app.handle.brush_editor_preview();
        if (!bytes || bytes.length === 0) return;
        if (bytes.length === lastLen && dataUrl) return;
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
        const next = URL.createObjectURL(blob);
        if (dataUrl) URL.revokeObjectURL(dataUrl);
        dataUrl = next;
        lastLen = bytes.length;
    }

    const compressor = new SignalCompressor(REFRESH_MS, () => {
        refresh();
        // After issuing a render request, poll for the async readback to
        // land. The first refresh() returns the prior cache (or empty);
        // subsequent frames see the new pixels once the scheduler
        // processes them in DarklyEngine::poll_pending.
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
        // Kick the engine's render loop so `poll_pending` processes the
        // in-flight readback. During UI interactions (slider drags),
        // `app._interactionCount` suppresses the continuous self-scheduling
        // render loop — discrete requests still run, so this keeps
        // readbacks advancing without competing for the main thread.
        app.requestFrame();
        refresh();
        scheduleFrame();
    }

    // Reactive trigger: graph snapshot, brush, or theme changes invalidate
    // the preview. The compressor debounces bursts (slider drags) into at
    // most one fire per REFRESH_MS. Display dimensions (`width`/`height`)
    // are intentionally NOT tracked — the engine renders at a fixed
    // canonical size and CSS scales, so resizing the dock should not
    // retrigger a render.
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
        if (dataUrl) URL.revokeObjectURL(dataUrl);
    });
</script>

<div class="brush-preview" style="--preview-w: {width}px; --preview-h: {height}px">
    {#if dataUrl}
        <img class="preview-img" src={dataUrl} alt="Brush preview" />
    {:else}
        <div class="preview-placeholder"></div>
    {/if}
</div>

<style>
    .brush-preview {
        position: relative;
        width: var(--preview-w);
        height: var(--preview-h);
        /* Docked against the bottom-right corner — round only the
         * inward-facing top-left corner for a clean edge. */
        border-radius: 4px 0 0 0;
        /* 80% opacity so the node graph behind the preview stays faintly
         * visible — matches the `preview-dock` overlay treatment. */
        background: color-mix(in srgb, var(--bg-hover) 80%, transparent);
        overflow: hidden;
        flex-shrink: 0;
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
