<script lang="ts">
    /**
     * Hue ring + saturation/value triangle. Owns its own HSV so a pick on the
     * ring survives the round trip through an achromatic host color: the
     * incoming `value` only replaces the wheel's state when it describes
     * different bytes (see `hsvFromColor`). Target agnostic: the host decides
     * what `value` is bound to and what the callbacks write.
     *
     * `oninput` fires per pointer move, `onchange` once on release, the
     * `live` / `commit` split the settings widgets use.
     */
    import { untrack } from 'svelte';
    import type { Color } from '../../state/app.svelte';
    import { hsvToRgb, type Hsv } from '../../lib/color';
    import {
        barycentricFor,
        hsvFromColor,
        hueAt,
        pointForHue,
        pointForSv,
        regionAt,
        svAt,
        triangleVertices,
        wheelGeometry,
        type WheelRegion,
    } from './wheel_model';

    let {
        value,
        oninput,
        onchange,
        size = 200,
    }: {
        value: Color;
        oninput: (c: Color) => void;
        onchange: (c: Color) => void;
        size?: number;
    } = $props();

    let geometry = $derived(wheelGeometry(size));

    // The triangle is painted, so it needs the real device grid: at CSS
    // resolution a HiDPI screen upscales it and the antialiased edges soften
    // back into steps. Re-read on resize, which is what fires when the window
    // moves to a display of a different density or the page zooms.
    let dpr = $state(typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1);
    let pixels = $derived(Math.max(1, Math.round(size * dpr)));

    let hsv = $state<Hsv>({ h: 0, s: 0, v: 0 });
    $effect(() => {
        const next = hsvFromColor(value, untrack(() => hsv));
        if (next !== untrack(() => hsv)) hsv = next;
    });

    let hueMarker = $derived(pointForHue(geometry, hsv.h));
    let svMarker = $derived(pointForSv(geometry, hsv.h, hsv.s, hsv.v));
    let hueRgb = $derived(hsvToRgb({ h: hsv.h, s: 1, v: 1 }, 255));

    let wheel: HTMLDivElement;
    let triangle: HTMLCanvasElement;
    // The region latched on pointerdown: a drag that leaves the ring keeps
    // steering the hue, a drag out of the triangle keeps steering s/v.
    let dragging: WheelRegion = null;

    // Painted per hue change: linear RGB over the barycentric weights is
    // exactly HSV at this hue, so the triangle is a plain per-pixel loop with
    // no trig inside. Each weight doubles as a signed distance from the edge
    // it vanishes on (`edgeScale`), which is the pixel's coverage: the edges
    // come out antialiased for the cost of the multiply.
    $effect(() => {
        const g = geometry;
        const h = hsv.h;
        const rgb = hueRgb;
        const ratio = dpr;
        const side = pixels;
        const ctx = triangle.getContext('2d');
        if (!ctx) return;
        const img = ctx.createImageData(side, side);
        const data = img.data;
        const bary = barycentricFor(g, h);
        const { hue, black, white } = triangleVertices(g, h);
        // A pixel of slack for the coverage band around each edge.
        const pad = 1 / ratio + 1;
        const lo = (v: number) => Math.max(0, Math.floor((v - pad) * ratio));
        const hi = (v: number) => Math.min(side - 1, Math.ceil((v + pad) * ratio));
        const minX = lo(Math.min(hue.x, black.x, white.x));
        const maxX = hi(Math.max(hue.x, black.x, white.x));
        const minY = lo(Math.min(hue.y, black.y, white.y));
        const maxY = hi(Math.max(hue.y, black.y, white.y));
        for (let py = minY; py <= maxY; py++) {
            const y = (py + 0.5) / ratio;
            for (let px = minX; px <= maxX; px++) {
                const w = bary.weights((px + 0.5) / ratio, y);
                const nearest = Math.min(w.hue, w.white, 1 - w.hue - w.white);
                // Distance to the nearest edge in device pixels, shifted so the
                // edge itself lands on half coverage.
                const coverage = nearest * bary.edgeScale * ratio + 0.5;
                if (coverage <= 0) continue;
                const wh = w.hue < 0 ? 0 : w.hue > 1 ? 1 : w.hue;
                const ww = w.white < 0 ? 0 : w.white > 1 ? 1 : w.white;
                const i = (py * side + px) * 4;
                data[i] = wh * rgb.r + ww * 255;
                data[i + 1] = wh * rgb.g + ww * 255;
                data[i + 2] = wh * rgb.b + ww * 255;
                data[i + 3] = coverage >= 1 ? 255 : coverage * 255;
            }
        }
        ctx.putImageData(img, 0, 0);
    });

    function local(e: PointerEvent): { x: number; y: number } {
        const r = wheel.getBoundingClientRect();
        const scale = geometry.size / r.width;
        return { x: (e.clientX - r.left) * scale, y: (e.clientY - r.top) * scale };
    }

    function steer(region: WheelRegion, e: PointerEvent) {
        const { x, y } = local(e);
        if (region === 'ring') hsv = { ...hsv, h: hueAt(geometry, x, y) };
        else if (region === 'triangle') hsv = { ...hsv, ...svAt(geometry, hsv.h, x, y) };
        oninput(hsvToRgb(hsv, value.a));
    }

    function onPointerDown(e: PointerEvent) {
        if (e.button !== 0) return;
        const { x, y } = local(e);
        const region = regionAt(geometry, hsv.h, x, y);
        if (!region) return;
        e.preventDefault();
        dragging = region;
        wheel.setPointerCapture(e.pointerId);
        steer(region, e);
    }

    function onPointerMove(e: PointerEvent) {
        if (dragging) steer(dragging, e);
    }

    function release(e: PointerEvent) {
        if (!dragging) return;
        dragging = null;
        if (wheel.hasPointerCapture(e.pointerId)) wheel.releasePointerCapture(e.pointerId);
        onchange(hsvToRgb(hsv, value.a));
    }
</script>

<svelte:window onresize={() => (dpr = window.devicePixelRatio || 1)} />

<div
    class="wheel"
    bind:this={wheel}
    style:width="{size}px"
    style:height="{size}px"
    style:--ring="{geometry.ringWidth}px"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={release}
    onpointercancel={release}
    role="slider"
    aria-label="Color wheel"
    aria-valuenow={Math.round(hsv.h)}
    aria-valuemin={0}
    aria-valuemax={360}
    tabindex="-1"
>
    <div class="ring"></div>
    <canvas class="triangle" bind:this={triangle} width={pixels} height={pixels}></canvas>
    <div class="marker hue" style:left="{hueMarker.x}px" style:top="{hueMarker.y}px"></div>
    <div class="marker sv" style:left="{svMarker.x}px" style:top="{svMarker.y}px"></div>
</div>

<style>
    .wheel {
        position: relative;
        flex: none;
        touch-action: none;
        user-select: none;
        cursor: crosshair;
        outline: none;
    }
    .ring,
    .triangle {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
    }
    .ring {
        border-radius: 50%;
        background: conic-gradient(from 90deg, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00);
        -webkit-mask: radial-gradient(
            circle,
            transparent calc(50% - var(--ring)),
            #000 calc(50% - var(--ring) + 0.5px)
        );
        mask: radial-gradient(circle, transparent calc(50% - var(--ring)), #000 calc(50% - var(--ring) + 0.5px));
    }
    /* The hue marker rides the ring, so it is sized to it: a fatter dot on a
       thin band reads as sitting on top of the wheel rather than in it. */
    .marker.hue {
        width: var(--ring);
        height: var(--ring);
    }
    .marker {
        position: absolute;
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 2px solid #fff;
        box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.6), inset 0 0 0 1px rgba(0, 0, 0, 0.6);
        transform: translate(-50%, -50%);
        pointer-events: none;
    }
</style>
