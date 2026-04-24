<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { theme } from '../../state/theme.svelte';
    import { rgbaToDataUrl } from '../layers/thumbnails';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        /** Target render / display dimensions, in CSS pixels. */
        width: number;
        height: number;
    }
    let { width, height }: Props = $props();

    /** Throttle window — matches Krita's 100ms FIRST_ACTIVE compressor for
     *  its live brush preview (kis_preset_live_preview_view.cpp:24). */
    const REFRESH_MS = 100;

    let dataUrl = $state('');
    /** Dimensions of the bytes that produced `dataUrl`. Kept separate from
     *  the current `width`/`height` so mid-resize we keep showing the last
     *  successful frame (at its native size) instead of flashing empty.
     *  Initialised to 0 — the `<img>` only renders once `dataUrl` is set,
     *  and `refresh()` writes real values before that happens. */
    let imgW = $state(0);
    let imgH = $state(0);
    /**
     * Bytes of the last accepted readback, used to skip redundant
     * data-URL encodes — the WASM call returns a fresh buffer every time
     * even when the underlying cache hasn't changed.
     */
    let lastHash = 0;

    function hashRgba(bytes: Uint8Array): number {
        // Sample a sparse grid — full-buffer hashing here costs more than the
        // data-URL encode we're trying to skip.
        let h = 2166136261 >>> 0;
        const step = Math.max(1, Math.floor(bytes.length / 256));
        for (let i = 0; i < bytes.length; i += step) {
            h = ((h ^ bytes[i]) * 16777619) >>> 0;
        }
        return h;
    }

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
        const w = width;
        const h = height;
        const rgba = app.handle.brush_editor_preview(w, h);
        if (!rgba || rgba.length === 0) return;
        // Guard against a readback that still holds the previous size —
        // during a resize burst the cached bytes can lag one frame behind.
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
        // After issuing a render request, poll for the async readback to
        // land. The first refresh() returns the prior cache (or zeros);
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

    // Reactive trigger: graph snapshot, preset, theme, or size changes all
    // invalidate the preview. The compressor debounces bursts (slider
    // drags, resize scrubs) into at most one fire per REFRESH_MS.
    $effect(() => {
        // Track dependencies — field reads establish the subscription;
        // actual values are consumed inside `refresh()` below.
        void brushGraph.graph;
        void brushGraph.activePreset;
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

<div class="brush-preview" style="--preview-w: {width}px; --preview-h: {height}px">
    {#if dataUrl}
        <img class="preview-img" src={dataUrl} alt="Brush preview" width={imgW} height={imgH} />
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
