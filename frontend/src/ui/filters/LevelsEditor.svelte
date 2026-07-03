<!--
  Levels editor — a histogram with a single input slider beneath it (black ● /
  gamma ▲ / white ●), bound to a Levels transfer `[inBlack, inWhite, gamma,
  outBlack, outWhite]`. Only the input triplet is edited here; the output range
  stays at its defaults. Values are normalized [0,1] except gamma (raw 0.1–10).

  Slider interaction is ported from Krita's `KisInputLevelsSliderWithGamma` /
  `KisLevelsSlider` (`libs/widgets/`, Krita team, GPL-3.0): gamma is stored as an
  exponent and its handle position is always derived, so moving black/white
  leaves gamma fixed while its handle slides proportionally between the bounds.
  The non-linear gamma↔position mappings live in `lib/levels_math.ts`.

  Emits `oninput` continuously while dragging and `onchange` on release, like
  `CurveEditor`; undo coalescing is handled by `PropertyAction::FilterParams`.
-->
<script lang="ts">
    import type { LevelsValues } from './filterParams';
    import {
        gammaHandlePos,
        gammaFromHandlePos,
        clampInputBlack,
        clampInputWhite,
        GAMMA_MIN,
    } from '../../lib/levels_math';

    interface Props {
        values: LevelsValues;
        onchange: (v: LevelsValues) => void;
        oninput?: (v: LevelsValues) => void;
        /**
         * 256 histogram bin counts for the selected channel, drawn behind the
         * slider. Absent (or empty) while the readback is pending.
         */
        histogram?: number[] | null;
    }

    let { values, onchange, oninput, histogram = null }: Props = $props();

    // Live-edit draft during a drag (mirrors CurveEditor); prop values otherwise.
    let draft = $state<LevelsValues | null>(null);
    const active = $derived(draft ?? values);

    const inBlack = $derived(active[0]);
    const inWhite = $derived(active[1]);
    const gamma = $derived(active[2]);
    const gammaPos = $derived(gammaHandlePos(inBlack, inWhite, gamma));

    type Handle = 'black' | 'gamma' | 'white';
    let dragging: Handle | null = null;
    let trackEl: HTMLDivElement;

    function normAt(clientX: number): number {
        const r = trackEl.getBoundingClientRect();
        return Math.max(0, Math.min(1, (clientX - r.left) / Math.max(1, r.width)));
    }

    function applyHandle(handle: Handle, n: number): LevelsValues {
        const [ib, iw, g, ob, ow] = active;
        if (handle === 'black') return [clampInputBlack(n, iw), iw, g, ob, ow];
        if (handle === 'white') return [ib, clampInputWhite(n, ib), g, ob, ow];
        return [ib, iw, gammaFromHandlePos(n, ib, iw), ob, ow];
    }

    function onDown(e: PointerEvent, handle: Handle) {
        e.stopPropagation();
        e.preventDefault();
        dragging = handle;
        draft = [...active] as LevelsValues;
        // Capture on the track so moves keep resolving even past the handle.
        trackEl.setPointerCapture(e.pointerId);
    }
    function onMove(e: PointerEvent) {
        if (!dragging) return;
        const next = applyHandle(dragging, normAt(e.clientX));
        draft = next;
        oninput?.(next);
    }
    function onUp(e: PointerEvent) {
        if (!dragging) return;
        trackEl.releasePointerCapture(e.pointerId);
        const next = draft ?? active;
        dragging = null;
        draft = null;
        onchange(next as LevelsValues);
    }

    // --- Histogram + groove canvases -----------------------------------------

    let histCanvas: HTMLCanvasElement | undefined = $state();
    let grooveCanvas: HTMLCanvasElement | undefined = $state();
    const CANVAS_W = 256;

    // Histogram bars for the selected channel, log-scaled so faint tails show.
    $effect(() => {
        const cv = histCanvas;
        if (!cv) return;
        const ctx = cv.getContext('2d');
        if (!ctx) return;
        ctx.clearRect(0, 0, cv.width, cv.height);
        const bins = histogram;
        if (!bins || bins.length === 0) return;
        const scaled = bins.map((c) => Math.log1p(c));
        const max = Math.max(1, ...scaled);
        const h = cv.height;
        const n = bins.length;
        ctx.fillStyle = 'rgba(255,255,255,0.55)';
        for (let i = 0; i < n; i++) {
            const bh = (scaled[i] / max) * h;
            ctx.fillRect((i / n) * cv.width, h - bh, cv.width / n, bh);
        }
    });

    // Groove: the tone the transfer produces across the input range — black
    // below inBlack, white above inWhite, pow(t, 1/gamma) between.
    $effect(() => {
        const cv = grooveCanvas;
        if (!cv) return;
        const ctx = cv.getContext('2d');
        if (!ctx) return;
        const img = ctx.createImageData(CANVAS_W, 1);
        const inv = 1 / Math.max(GAMMA_MIN, gamma);
        for (let px = 0; px < CANVAS_W; px++) {
            const x = px / (CANVAS_W - 1);
            let v: number;
            if (x <= inBlack) v = 0;
            else if (x >= inWhite) v = 1;
            else v = Math.pow((x - inBlack) / Math.max(1e-6, inWhite - inBlack), inv);
            const g = Math.round(v * 255);
            img.data.set([g, g, g, 255], px * 4);
        }
        ctx.putImageData(img, 0, 0);
    });
</script>

<div class="levels-editor">
    <div class="hist-area">
        <canvas bind:this={histCanvas} width={CANVAS_W} height="64" class="hist"></canvas>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="track" bind:this={trackEl} onpointermove={onMove} onpointerup={onUp}>
        <canvas bind:this={grooveCanvas} width={CANVAS_W} height="1" class="groove"></canvas>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle black"
            style="left: {inBlack * 100}%"
            onpointerdown={(e) => onDown(e, 'black')}
            title="Black point"
        ></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle gamma"
            style="left: {gammaPos * 100}%"
            onpointerdown={(e) => onDown(e, 'gamma')}
            title="Gamma"
        ></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle white"
            style="left: {inWhite * 100}%"
            onpointerdown={(e) => onDown(e, 'white')}
            title="White point"
        ></div>
    </div>
</div>

<style>
    .levels-editor {
        display: flex;
        flex-direction: column;
    }
    .hist-area {
        height: 64px;
        background: color-mix(in srgb, var(--bg) 80%, black);
        border-radius: 3px 3px 0 0;
        overflow: hidden;
    }
    .hist {
        display: block;
        width: 100%;
        height: 100%;
    }
    .track {
        position: relative;
        height: 16px;
        cursor: ew-resize;
        touch-action: none;
    }
    .groove {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 7px;
        border-radius: 0 0 3px 3px;
    }
    /* Diamond handles with a dark+light double ring so every fill stays visible
       on the histogram / groove regardless of the tone behind it. */
    .handle {
        position: absolute;
        top: 3.5px;
        width: 9px;
        height: 9px;
        transform: translateX(-50%) rotate(45deg);
        box-shadow:
            0 0 0 1px rgba(0, 0, 0, 0.9),
            0 0 0 2px rgba(255, 255, 255, 0.85);
        cursor: ew-resize;
    }
    .handle.black {
        background: #111;
    }
    .handle.white {
        background: #fff;
    }
    .handle.gamma {
        background: var(--accent);
    }
</style>
