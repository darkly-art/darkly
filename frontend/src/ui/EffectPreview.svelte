<script lang="ts">
    import { onDestroy } from 'svelte';
    import { app } from '../state/app.svelte';
    import Icon from '../icons/Icon.svelte';
    import { pollPreview, showsPreview, type PreviewData } from './preview_frames';

    /** What a picker card knows about its entry — the subset of a catalog entry
     *  this component needs to decide what to show. */
    export interface PreviewEntry {
        type: string;
        displayName?: string;
        icon?: string | null;
        supportsPreview?: boolean;
    }

    let { catalog, entry }: { catalog: string; entry: PreviewEntry } = $props();

    /** Frames during which we actively poll WASM for the async readbacks to
     *  land. Generation is paced across engine ticks; this is a generous
     *  ceiling. */
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

    async function tick(now: number) {
        rafHandle = 0;
        if (!data) {
            // Still generating — kick the engine's render loop so it pumps the
            // next slice of frames and drains the landed readbacks, then check.
            if (framesRemaining <= 0 || !app.engine) return;
            framesRemaining--;
            app.requestFrame();
            const pd = await pollPreview(app.engine, catalog, entry.type);
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

    // Kick off a fresh render whenever the entry changes. No caching — the
    // engine re-renders against the current document each time the picker opens.
    $effect(() => {
        void catalog;
        void entry.type;
        if (!showsPreview(entry)) return;
        frameIdx = 0;
        lastDrawn = -1;
        accum = 0;
        prevTime = 0;
        data = null;
        framesRemaining = POLL_FRAMES;
        app.engine?.api.startPreview({ catalog, type: entry.type });
        schedule();
    });

    onDestroy(() => {
        if (rafHandle) cancelAnimationFrame(rafHandle);
    });
</script>

<!-- Preview, then icon, then a named placeholder. The chain has to be total:
     veils deliberately carry no icon (their picker renders a live thumbnail),
     so a future veil that declared no preview would otherwise render an empty
     card with nothing to say why. -->
{#if showsPreview(entry)}
    <!-- Intrinsic size follows the document so the card holds the canvas aspect
         ratio from the start (placeholder uses doc dims, real frames match). The
         element scales to the card width; height follows proportionally. -->
    <canvas
        class="effect-preview"
        bind:this={canvasEl}
        width={data?.width ?? app.docW}
        height={data?.height ?? app.docH}
        class:loading={!data}
    ></canvas>
{:else if entry.icon}
    <div class="effect-preview placeholder">
        <Icon name={entry.icon} />
    </div>
{:else}
    <div class="effect-preview placeholder">
        <span class="placeholder-name">{entry.displayName ?? entry.type}</span>
    </div>
{/if}

<style>
    .effect-preview {
        display: block;
        width: 100%;
        height: auto;
        border-radius: var(--radius-sm);
        background: var(--bg);
        image-rendering: auto;
    }
    /* Faint shimmer while the engine renders the frames. */
    .effect-preview.loading {
        background-image: linear-gradient(
            45deg,
            color-mix(in srgb, var(--accent) 20%, transparent) 0%,
            transparent 70%
        );
    }
    /* Fallback box: a centered iconify glyph, or the entry's name when it
       declares neither a preview nor an icon. */
    .placeholder {
        aspect-ratio: 16 / 9;
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 32px;
        color: var(--text-dim);
    }
    .placeholder-name {
        font-size: 12px;
        text-align: center;
        padding: 0 4px;
    }
</style>
