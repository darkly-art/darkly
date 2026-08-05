<script lang="ts">
    import { onDestroy } from 'svelte';
    import { app } from '../state/app.svelte';
    import Icon from '../icons/Icon.svelte';
    import {
        pollPreview,
        showsPreview,
        type PreviewData,
        type PreviewVariant,
    } from './preview_frames';

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
    /** The one frame shown at rest — a card asks for this on mount, so opening
     *  a picker costs one frame per card rather than a whole sequence each. */
    let still = $state<PreviewData | null>(null);
    /** The sequence, requested the first time the pointer arrives and kept for
     *  the life of the card, so a second hover replays rather than regenerates. */
    let anim = $state<PreviewData | null>(null);
    let hovering = $state(false);

    let rafHandle = 0;
    let framesRemaining = 0;
    /** Which variant the current poll loop is waiting on, or `null` when it is
     *  only playing back. */
    let awaiting: PreviewVariant | null = null;
    // Playback cursor.
    let frameIdx = 0;
    let lastDrawn: ImageData | null = null;
    let accum = 0;
    let prevTime = 0;

    /** What is on screen: the sequence while hovering and loaded, else the
     *  still. Falling back to the still means the hand-off costs nothing
     *  visually — the sequence's own frame at this point *is* the still. */
    function current(): ImageData | null {
        if (hovering && anim) return anim.frames[frameIdx % anim.frames.length];
        return still?.frames[0] ?? null;
    }

    function draw() {
        const frame = current();
        if (!canvasEl || !frame || frame === lastDrawn) return;
        const ctx = canvasEl.getContext('2d');
        if (!ctx) return;
        ctx.putImageData(frame, 0, 0);
        lastDrawn = frame;
    }

    /** Ask the engine for `variant` and start polling for it. */
    function request(variant: PreviewVariant) {
        awaiting = variant;
        framesRemaining = POLL_FRAMES;
        app.engine?.api.startPreview({ catalog, type: entry.type, variant });
        schedule();
    }

    async function tick(now: number) {
        rafHandle = 0;
        if (awaiting) {
            // Still generating — kick the engine's render loop so it pumps the
            // next slice of frames and drains the landed readbacks, then check.
            if (framesRemaining <= 0 || !app.engine) {
                awaiting = null;
                schedule();
                return;
            }
            framesRemaining--;
            app.requestFrame();
            const wanted = awaiting;
            const pd = await pollPreview(app.engine, catalog, entry.type, wanted);
            if (pd && awaiting === wanted) {
                awaiting = null;
                if (wanted === 'still') still = pd;
                else {
                    anim = pd;
                    prevTime = now;
                }
                // Deliberately not drawn here. The canvas sizes itself from the
                // frames that landed, and assigning a canvas's `width` or
                // `height` clears it — so painting in the same turn as the state
                // change races Svelte's flush and loses. The next tick runs after
                // that flush, which is where the first frame goes on screen.
                lastDrawn = null;
            }
            schedule();
            return;
        }

        // Playback: advance frames on the sequence's own fps clock, and only
        // while the pointer is over the card. A single-frame sequence holds.
        if (hovering && anim && anim.frames.length > 1) {
            const dt = prevTime > 0 ? (now - prevTime) / 1000 : 0;
            prevTime = now;
            accum += dt;
            const frameDur = 1 / anim.fps;
            while (accum >= frameDur) {
                accum -= frameDur;
                frameIdx = (frameIdx + 1) % anim.frames.length;
            }
            draw();
            schedule();
            return;
        }
        draw();
    }

    function schedule() {
        if (rafHandle) return;
        rafHandle = requestAnimationFrame(tick);
    }

    function enter() {
        hovering = true;
        prevTime = 0;
        accum = 0;
        frameIdx = 0;
        // The sequence is generated on demand and then kept: hovering the same
        // card twice replays what is already in hand.
        if (!anim && !awaiting) request('animated');
        else schedule();
    }

    function leave() {
        hovering = false;
        // Abandon a sequence still in flight — the pointer has gone, and the
        // engine drops the job when nothing polls it.
        if (awaiting === 'animated') awaiting = null;
        schedule();
    }

    // Kick off a fresh still whenever the entry changes. No caching — the engine
    // re-renders against the current document each time the picker opens.
    $effect(() => {
        void catalog;
        void entry.type;
        if (!showsPreview(entry)) return;
        still = null;
        anim = null;
        hovering = false;
        frameIdx = 0;
        lastDrawn = null;
        accum = 0;
        prevTime = 0;
        request('still');
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
         element scales to the card width; height follows proportionally.

         Pointer rather than mouse events: `pointerenter` / `pointerleave` have
         no keyboard-equivalent a11y requirement, and animating on hover is
         decoration over a control that is already reachable and labelled. -->
    <canvas
        class="effect-preview"
        bind:this={canvasEl}
        width={still?.width ?? anim?.width ?? app.docW}
        height={still?.height ?? anim?.height ?? app.docH}
        class:loading={!still && !anim}
        onpointerenter={enter}
        onpointerleave={leave}
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
    /* Faint shimmer while the engine renders the still. */
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
