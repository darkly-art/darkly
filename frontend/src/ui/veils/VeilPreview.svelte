<script lang="ts">
    import { onDestroy } from 'svelte';
    import { app } from '../../state/app.svelte';
    import {
        getOrStartPreview,
        pollPreview,
        type PreviewData,
    } from './veil_preview_cache';

    let { veilType }: { veilType: string } = $props();

    /** Frames during which we actively poll WASM for the async readbacks to
     *  land. Generation is a handful of frames; this is a generous ceiling. */
    const POLL_FRAMES = 180;

    let canvasEl = $state<HTMLCanvasElement | null>(null);
    let data = $state<PreviewData | null>(null);

    let rafHandle = 0;
    let framesRemaining = 0;
    // Playback cursor.
    let frameIdx = 0;
    let lastDrawn = -1;
    let accum = 0;
    let prevTime = 0;

    function draw() {
        if (!canvasEl || !data || frameIdx === lastDrawn) return;
        const ctx = canvasEl.getContext('2d');
        if (!ctx) return;
        ctx.putImageData(data.frames[frameIdx], 0, 0);
        lastDrawn = frameIdx;
    }

    function tick(now: number) {
        rafHandle = 0;
        if (!data) {
            // Still generating — kick the engine's render loop so its
            // `poll_pending` drains the in-flight readbacks, then check.
            if (framesRemaining <= 0 || !app.handle) return;
            framesRemaining--;
            app.requestFrame();
            const pd = pollPreview(app.handle, veilType);
            if (pd) {
                data = pd;
                prevTime = now;
            }
            schedule();
            return;
        }

        // Playback: advance frames on the data's fps clock. A single-frame
        // (static) preview just holds on frame 0.
        if (data.frames.length > 1) {
            const dt = prevTime > 0 ? (now - prevTime) / 1000 : 0;
            prevTime = now;
            accum += dt;
            const frameDur = 1 / data.fps;
            while (accum >= frameDur) {
                accum -= frameDur;
                frameIdx = (frameIdx + 1) % data.frames.length;
            }
        }
        draw();
        schedule();
    }

    function schedule() {
        if (rafHandle) return;
        rafHandle = requestAnimationFrame(tick);
    }

    // Kick off (or adopt the cache for) the preview whenever the type changes.
    $effect(() => {
        void veilType;
        frameIdx = 0;
        lastDrawn = -1;
        accum = 0;
        prevTime = 0;
        const cached = app.handle ? getOrStartPreview(app.handle, veilType) : null;
        if (cached) {
            data = cached;
        } else {
            data = null;
            framesRemaining = POLL_FRAMES;
        }
        schedule();
    });

    onDestroy(() => {
        if (rafHandle) cancelAnimationFrame(rafHandle);
    });
</script>

<canvas
    class="veil-preview"
    bind:this={canvasEl}
    width={data?.width ?? 256}
    height={data?.height ?? 144}
    class:loading={!data}
></canvas>

<style>
    .veil-preview {
        display: block;
        width: 100%;
        aspect-ratio: 16 / 9;
        border-radius: var(--radius-sm);
        background: var(--bg);
        image-rendering: auto;
    }
    /* Faint shimmer while the engine renders the frames. */
    .veil-preview.loading {
        background-image: linear-gradient(
            45deg,
            color-mix(in srgb, var(--accent) 20%, transparent) 0%,
            transparent 70%
        );
    }
</style>
