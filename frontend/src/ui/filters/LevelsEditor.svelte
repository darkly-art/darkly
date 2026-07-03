<!--
  Levels editor — a three-handle input slider (black ● / gamma ▲ / white ●), a
  two-handle output slider, and numeric spinboxes, bound to a Levels transfer
  `[inBlack, inWhite, gamma, outBlack, outWhite]` (all normalized [0,1] except
  gamma, the raw 0.1–10 exponent).

  The slider interaction is ported from Krita's `KisInputLevelsSliderWithGamma`
  / `KisLevelsSlider` (`libs/widgets/`, Krita team, GPL-3.0): gamma is stored as
  an exponent and its handle position is always derived, so moving the black or
  white bound leaves gamma fixed while its handle slides proportionally. The
  non-linear gamma↔position mappings live in `lib/levels_math.ts`.

  Emits `oninput` continuously while dragging and `onchange` on release, exactly
  like `CurveEditor` — undo coalescing is handled downstream by
  `PropertyAction::FilterParams`.
-->
<script lang="ts">
    import type { LevelsValues } from './filterParams';
    import {
        gammaHandlePos,
        gammaFromHandlePos,
        clampInputBlack,
        clampInputWhite,
        clampOutput,
        GAMMA_MIN,
        GAMMA_MAX,
    } from '../../lib/levels_math';

    interface Props {
        values: LevelsValues;
        onchange: (v: LevelsValues) => void;
        oninput?: (v: LevelsValues) => void;
        /**
         * 256 histogram bin counts for the selected channel, drawn above the
         * input slider. Absent (or empty) while the readback is pending.
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
    const outBlack = $derived(active[3]);
    const outWhite = $derived(active[4]);
    const gammaPos = $derived(gammaHandlePos(inBlack, inWhite, gamma));

    type Handle = 'inBlack' | 'gamma' | 'inWhite' | 'outBlack' | 'outWhite';
    let dragging: Handle | null = null;
    let trackEl: HTMLDivElement;
    let outTrackEl: HTMLDivElement;

    function normAt(el: HTMLDivElement, clientX: number): number {
        const r = el.getBoundingClientRect();
        return Math.max(0, Math.min(1, (clientX - r.left) / Math.max(1, r.width)));
    }

    // Produce the next transfer for a handle at normalized position `n`.
    function applyHandle(handle: Handle, n: number): LevelsValues {
        const [ib, iw, g, ob, ow] = active;
        switch (handle) {
            case 'inBlack':
                return [clampInputBlack(n, iw), iw, g, ob, ow];
            case 'inWhite':
                return [ib, clampInputWhite(n, ib), g, ob, ow];
            case 'gamma':
                return [ib, iw, gammaFromHandlePos(n, ib, iw), ob, ow];
            case 'outBlack':
                return [ib, iw, g, clampOutput(n), ow];
            case 'outWhite':
                return [ib, iw, g, ob, clampOutput(n)];
        }
    }

    function onHandleDown(e: PointerEvent, handle: Handle) {
        e.stopPropagation();
        e.preventDefault();
        dragging = handle;
        draft = [...active] as LevelsValues;
        (e.currentTarget as Element).setPointerCapture(e.pointerId);
    }
    function onHandleMove(e: PointerEvent) {
        if (!dragging) return;
        const el = dragging === 'outBlack' || dragging === 'outWhite' ? outTrackEl : trackEl;
        const next = applyHandle(dragging, normAt(el, e.clientX));
        draft = next;
        oninput?.(next);
    }
    function onHandleUp(e: PointerEvent) {
        if (!dragging) return;
        (e.currentTarget as Element).releasePointerCapture(e.pointerId);
        const next = draft ?? active;
        dragging = null;
        draft = null;
        onchange(next as LevelsValues);
    }

    // --- Spinboxes (0–255 for the four in/out bounds, raw for gamma) ---------

    function setField(idx: number, v: number) {
        const next = [...active] as LevelsValues;
        next[idx] = v;
        // Re-apply the same cross/clamp rules the handles enforce.
        next[0] = clampInputBlack(next[0], next[1]);
        next[1] = clampInputWhite(next[1], next[0]);
        next[3] = clampOutput(next[3]);
        next[4] = clampOutput(next[4]);
        next[2] = Math.max(GAMMA_MIN, Math.min(GAMMA_MAX, next[2]));
        onchange(next);
    }
    const to255 = (v: number) => Math.round(v * 255);
    const from255 = (v: number) => v / 255;

    // --- Groove + histogram canvases -----------------------------------------

    let grooveCanvas: HTMLCanvasElement | undefined = $state();
    let histCanvas: HTMLCanvasElement | undefined = $state();
    const CANVAS_W = 256;

    // Input groove: the tone the transfer produces across the input range —
    // black below inBlack, white above inWhite, pow(t, 1/gamma) between.
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
        ctx.fillStyle = 'rgba(255,255,255,0.55)';
        const n = bins.length;
        for (let i = 0; i < n; i++) {
            const bh = (scaled[i] / max) * h;
            const x = (i / n) * cv.width;
            ctx.fillRect(x, h - bh, cv.width / n, bh);
        }
    });
</script>

<div class="levels-editor">
    <div class="hist-wrap">
        <canvas bind:this={histCanvas} width={CANVAS_W} height="40" class="hist"></canvas>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="track input-track"
        bind:this={trackEl}
        onpointermove={onHandleMove}
        onpointerup={onHandleUp}
    >
        <canvas bind:this={grooveCanvas} width={CANVAS_W} height="1" class="groove"></canvas>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle black"
            style="left: {inBlack * 100}%"
            onpointerdown={(e) => onHandleDown(e, 'inBlack')}
            title="Input black"
        ></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle gamma"
            style="left: {gammaPos * 100}%"
            onpointerdown={(e) => onHandleDown(e, 'gamma')}
            title="Gamma"
        ></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle white"
            style="left: {inWhite * 100}%"
            onpointerdown={(e) => onHandleDown(e, 'inWhite')}
            title="Input white"
        ></div>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="track output-track"
        bind:this={outTrackEl}
        onpointermove={onHandleMove}
        onpointerup={onHandleUp}
    >
        <div class="out-groove"></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle black"
            style="left: {outBlack * 100}%"
            onpointerdown={(e) => onHandleDown(e, 'outBlack')}
            title="Output black"
        ></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="handle white"
            style="left: {outWhite * 100}%"
            onpointerdown={(e) => onHandleDown(e, 'outWhite')}
            title="Output white"
        ></div>
    </div>

    <div class="spinboxes">
        <label class="spin">
            <span>In ▾</span>
            <input
                type="number" min="0" max="255" step="1"
                value={to255(inBlack)}
                onchange={(e) => setField(0, from255(+(e.target as HTMLInputElement).value))}
            />
        </label>
        <label class="spin">
            <span>γ</span>
            <input
                type="number" min={GAMMA_MIN} max={GAMMA_MAX} step="0.01"
                value={gamma.toFixed(3)}
                onchange={(e) => setField(2, +(e.target as HTMLInputElement).value)}
            />
        </label>
        <label class="spin">
            <span>In ▴</span>
            <input
                type="number" min="0" max="255" step="1"
                value={to255(inWhite)}
                onchange={(e) => setField(1, from255(+(e.target as HTMLInputElement).value))}
            />
        </label>
        <label class="spin">
            <span>Out ▾</span>
            <input
                type="number" min="0" max="255" step="1"
                value={to255(outBlack)}
                onchange={(e) => setField(3, from255(+(e.target as HTMLInputElement).value))}
            />
        </label>
        <label class="spin">
            <span>Out ▴</span>
            <input
                type="number" min="0" max="255" step="1"
                value={to255(outWhite)}
                onchange={(e) => setField(4, from255(+(e.target as HTMLInputElement).value))}
            />
        </label>
    </div>
</div>

<style>
    .levels-editor {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .hist-wrap {
        height: 40px;
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
        height: 14px;
        border-radius: 3px;
        overflow: visible;
    }
    .input-track {
        margin-top: -6px;
    }
    .groove,
    .out-groove {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        border-radius: 3px;
    }
    .groove {
        image-rendering: auto;
    }
    .out-groove {
        background: linear-gradient(to right, #000, #fff);
    }
    .handle {
        position: absolute;
        top: 50%;
        width: 10px;
        height: 10px;
        transform: translate(-50%, -50%) rotate(45deg);
        border: 1px solid var(--bg);
        cursor: ew-resize;
        box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.4);
    }
    .handle.black {
        background: #000;
    }
    .handle.white {
        background: #fff;
    }
    .handle.gamma {
        background: var(--accent);
        border-radius: 2px;
    }
    .spinboxes {
        display: flex;
        gap: 4px;
        justify-content: space-between;
    }
    .spin {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 2px;
        font-size: 9px;
        color: var(--text-muted);
        flex: 1;
        min-width: 0;
    }
    .spin input {
        width: 100%;
        min-width: 0;
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        font-size: 10px;
        padding: 1px 2px;
        text-align: center;
    }
</style>
