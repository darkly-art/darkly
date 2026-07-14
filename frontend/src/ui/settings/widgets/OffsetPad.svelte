<script lang="ts">
    import { padPointToOffset, offsetToPadPoint, offsetPolar } from '../../../lib/offsetPad';

    // A square pad with a crosshair center and a draggable diamond handle. The
    // handle's direction from center is the offset direction; its distance is the
    // magnitude, mapped so the pad edge = the param's `max` radius. Double-click
    // resets to center. Pure mapping math lives in `lib/offsetPad.ts`.
    type Props = {
        value: [number, number];
        max: number;
        size?: number;
        oninput?: (v: [number, number]) => void;
        onchange?: (v: [number, number]) => void;
    };
    let { value, max, size = 84, oninput, onchange }: Props = $props();

    let padEl: HTMLDivElement;
    let dragging = $state(false);

    const handle = $derived(offsetToPadPoint(value, size, max));
    const polar = $derived(offsetPolar(value));

    function offsetFromEvent(e: PointerEvent): [number, number] {
        const rect = padEl.getBoundingClientRect();
        const px = ((e.clientX - rect.left) / rect.width) * size;
        const py = ((e.clientY - rect.top) / rect.height) * size;
        return padPointToOffset(px, py, size, max);
    }

    function startDrag(e: PointerEvent) {
        e.preventDefault();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        dragging = true;
        oninput?.(offsetFromEvent(e));
    }
    function moveDrag(e: PointerEvent) {
        if (!dragging) return;
        oninput?.(offsetFromEvent(e));
    }
    function endDrag(e: PointerEvent) {
        if (!dragging) return;
        dragging = false;
        (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
        onchange?.(offsetFromEvent(e));
    }
    function reset() {
        onchange?.([0, 0]);
    }
</script>

<div class="offset-pad">
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
        class="pad"
        class:dragging
        bind:this={padEl}
        style:width="{size}px"
        style:height="{size}px"
        role="slider"
        tabindex="0"
        aria-valuemin={0}
        aria-valuemax={max}
        aria-valuenow={polar.distance}
        onpointerdown={startDrag}
        onpointermove={moveDrag}
        onpointerup={endDrag}
        onpointercancel={endDrag}
        ondblclick={reset}
    >
        <div class="cross-h"></div>
        <div class="cross-v"></div>
        <svg class="connector" width={size} height={size}>
            <line x1={size / 2} y1={size / 2} x2={handle[0]} y2={handle[1]} />
        </svg>
        <div class="handle" style:left="{handle[0]}px" style:top="{handle[1]}px"></div>
    </div>
    <span class="readout">{Math.round(polar.angle)}° · {polar.distance.toFixed(1)}px</span>
</div>

<style>
    .offset-pad {
        display: flex;
        align-items: center;
        gap: 10px;
    }
    .pad {
        position: relative;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        cursor: crosshair;
        touch-action: none;
        flex: none;
    }
    .pad.dragging {
        cursor: grabbing;
    }
    .cross-h,
    .cross-v {
        position: absolute;
        background: var(--bg-hover);
    }
    .cross-h {
        left: 6px;
        right: 6px;
        top: 50%;
        height: 1px;
        transform: translateY(-0.5px);
    }
    .cross-v {
        top: 6px;
        bottom: 6px;
        left: 50%;
        width: 1px;
        transform: translateX(-0.5px);
    }
    .connector {
        position: absolute;
        inset: 0;
        pointer-events: none;
    }
    .connector line {
        stroke: var(--accent);
        stroke-width: 1;
        opacity: 0.5;
    }
    .handle {
        position: absolute;
        width: 10px;
        height: 10px;
        background: #ffffff;
        border: 1px solid var(--accent);
        transform: translate(-50%, -50%) rotate(45deg);
        box-shadow: 0 1px 2px rgba(0, 0, 0, 0.4);
        pointer-events: none;
    }
    .pad:hover .handle,
    .pad:focus-visible .handle,
    .pad.dragging .handle {
        box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 30%, transparent);
    }
    .readout {
        font-size: 11px;
        color: var(--text-muted);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
</style>
