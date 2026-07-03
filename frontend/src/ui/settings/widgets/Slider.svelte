<script lang="ts">
    import { resolveStep, clampValue, valueToFraction, fractionToValue } from '../../../lib/slider';

    type Props = {
        value: number;
        min: number;
        max: number;
        step?: number;
        integer?: boolean;
        disabled?: boolean;
        onchange: (v: number) => void;
        /** When provided, the readout is a read-only formatted label instead of
         *  an editable number field. Use for compact property-panel rows
         *  (e.g. `v => Math.round(v * 100) + '%'`). */
        format?: (v: number) => string;
    };
    let {
        value,
        min,
        max,
        step,
        integer = false,
        disabled = false,
        onchange,
        format,
    }: Props = $props();

    const resolvedStep = $derived(resolveStep(min, max, integer, step));
    const fraction = $derived(valueToFraction(value, min, max));

    let trackEl: HTMLDivElement;
    let dragging = $state(false);

    function emit(v: number) {
        if (!Number.isFinite(v)) return;
        onchange(clampValue(v, min, max, integer));
    }

    function valueFromClientX(clientX: number): number {
        const rect = trackEl.getBoundingClientRect();
        const f = rect.width > 0 ? (clientX - rect.left) / rect.width : 0;
        return fractionToValue(f, min, max, integer, step);
    }

    function startDrag(e: PointerEvent) {
        if (disabled) return;
        e.preventDefault();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        dragging = true;
        onchange(valueFromClientX(e.clientX));
    }
    function moveDrag(e: PointerEvent) {
        if (!dragging) return;
        onchange(valueFromClientX(e.clientX));
    }
    function endDrag(e: PointerEvent) {
        if (!dragging) return;
        dragging = false;
        (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    }

    function onKey(e: KeyboardEvent) {
        if (disabled) return;
        let v: number;
        switch (e.key) {
            case 'ArrowLeft':
            case 'ArrowDown':
                v = value - resolvedStep;
                break;
            case 'ArrowRight':
            case 'ArrowUp':
                v = value + resolvedStep;
                break;
            case 'Home':
                v = min;
                break;
            case 'End':
                v = max;
                break;
            default:
                return;
        }
        e.preventDefault();
        emit(v);
    }
</script>

<div class="slider" class:disabled>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
        class="control"
        class:dragging
        role="slider"
        tabindex={disabled ? -1 : 0}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={value}
        aria-disabled={disabled}
        onpointerdown={startDrag}
        onpointermove={moveDrag}
        onpointerup={endDrag}
        onpointercancel={endDrag}
        onkeydown={onKey}
    >
        <div class="track" bind:this={trackEl}>
            <div class="fill" style:width="{fraction * 100}%"></div>
            <div class="handle" style:left="{fraction * 100}%"></div>
        </div>
    </div>

    {#if format}
        <span class="readout">{format(value)}</span>
    {:else}
        <input
            type="number"
            class="num"
            {min}
            {max}
            {disabled}
            step={resolvedStep}
            {value}
            onchange={(e) => emit(e.currentTarget.valueAsNumber)}
        />
    {/if}
</div>

<style>
    .slider {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
    }

    /* The interactive area is padded top/bottom so the whole ~24px row is a
       grab target, even though the visible groove is only 8px tall. */
    .control {
        flex: 1;
        min-width: 0;
        padding: 8px 3px;
        cursor: pointer;
        touch-action: none;
        outline: none;
    }
    .control.dragging {
        cursor: grabbing;
    }

    .track {
        position: relative;
        height: 8px;
        border-radius: var(--radius-sm);
        background: var(--bg-active);
    }

    .fill {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        background: var(--accent);
        border-radius: var(--radius-sm) 0 0 var(--radius-sm);
    }

    .handle {
        position: absolute;
        top: 50%;
        width: 6px;
        height: 20px;
        border-radius: 3px;
        background: #ffffff;
        transform: translate(-50%, -50%);
        box-shadow: 0 1px 2px rgba(0, 0, 0, 0.4);
        transition: box-shadow 0.1s;
    }
    .control:hover .handle,
    .control:focus-visible .handle,
    .control.dragging .handle {
        box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 30%, transparent);
    }

    .readout {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 40px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .num {
        width: 64px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: var(--radius-sm);
        padding: 4px 6px;
        font-size: 12px;
    }
    .num:focus {
        outline: 2px solid var(--accent);
        outline-offset: 0;
        border-color: transparent;
    }

    .slider.disabled {
        opacity: 0.4;
    }
    .slider.disabled .control {
        cursor: not-allowed;
    }
</style>
