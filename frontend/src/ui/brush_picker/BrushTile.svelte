<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { theme } from '../../state/theme.svelte';
    import type { BrushInfo } from '../../state/brush_graph.svelte';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        brush: BrushInfo;
        active?: boolean;
        onSelect: (brush: BrushInfo) => void;
    }
    let { brush, active = false, onSelect }: Props = $props();

    /** Same throttle cadence as the dab and editor previews. */
    const REFRESH_MS = 100;

    /** Object URL pointing at the most recent baked PNG bytes. Object
     *  URLs are cheaper than data URLs across remounts (we hand the
     *  browser bytes once, not on every render), and they're trivially
     *  revoked when bytes change or the tile unmounts. */
    let objectUrl = $state('');
    /** Track the bytes that produced `objectUrl` so we can skip redundant
     *  Blob/URL churn on cache hits. */
    let lastByteLength = 0;

    /** rAF poll budget — single-dab thumbnails bake fast; 30 frames is
     *  more than enough for the readback to land. */
    const POLL_FRAMES_PER_REQUEST = 30;
    let framesRemaining = 0;
    let rafHandle = 0;

    function refresh() {
        if (!app.handle) return;
        const png = app.handle.brush_thumbnail(brush.name);
        if (!png || png.length === 0) return;
        // Same bytes as last time — skip the Blob/URL churn.
        if (png.length === lastByteLength && objectUrl) return;
        lastByteLength = png.length;
        const blob = new Blob([new Uint8Array(png)], { type: 'image/png' });
        const next = URL.createObjectURL(blob);
        if (objectUrl) URL.revokeObjectURL(objectUrl);
        objectUrl = next;
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
        app.requestFrame();
        refresh();
        scheduleFrame();
    }

    // Reactive trigger: WASM handle becoming available and theme swaps
    // both require a fresh thumbnail. The brush prop is per-tile and
    // doesn't change for a mounted instance, but include it so the
    // effect's dependency set is explicit.
    $effect(() => {
        void app.handle;
        void theme.current;
        void brush.name;
        untrack(() => compressor.request());
    });

    onDestroy(() => {
        compressor.cancel();
        if (rafHandle) cancelAnimationFrame(rafHandle);
        if (objectUrl) URL.revokeObjectURL(objectUrl);
    });
</script>

<button
    class="brush-tile"
    class:active
    onclick={() => onSelect(brush)}
    title={brush.description || brush.name}
>
    <div class="thumb">
        {#if objectUrl}
            <img src={objectUrl} alt="" />
        {/if}
    </div>
    <span class="name">{brush.name}</span>
</button>

<style>
    .brush-tile {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        text-align: left;
        transition: background 0.1s, border-color 0.1s;
    }
    .brush-tile:hover {
        background: var(--bg-active);
    }
    .brush-tile.active {
        border-color: var(--accent);
    }
    .thumb {
        width: 100%;
        aspect-ratio: 320 / 120;
        background: var(--bg);
        border-radius: 4px;
        overflow: hidden;
    }
    .thumb img {
        width: 100%;
        height: 100%;
        display: block;
        image-rendering: auto;
    }
    .name {
        font-size: 11px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
