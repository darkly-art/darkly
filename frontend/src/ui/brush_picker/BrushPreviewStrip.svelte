<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { theme } from '../../state/theme.svelte';
    import { SignalCompressor } from '../../lib/signal_compressor';

    interface Props {
        /** Library brush name to look up in the engine's baked PNG cache. */
        brushName: string;
    }
    let { brushName }: Props = $props();

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

    function refresh() {
        if (!app.handle) return;
        const stroke = loadPng(
            app.handle.brush_thumbnail(brushName),
            strokeUrl,
            lastStrokeLen,
        );
        if (stroke) {
            strokeUrl = stroke.url;
            lastStrokeLen = stroke.len;
        }
        const dab = loadPng(
            app.handle.brush_dab_thumbnail(brushName),
            dabUrl,
            lastDabLen,
        );
        if (dab) {
            dabUrl = dab.url;
            lastDabLen = dab.len;
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
        app.requestFrame();
        refresh();
        scheduleFrame();
    }

    // Reactive trigger: WASM handle becoming available, theme swaps,
    // and the brush name changing all require fresh thumbnails.
    $effect(() => {
        void app.handle;
        void theme.current;
        void brushName;
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
