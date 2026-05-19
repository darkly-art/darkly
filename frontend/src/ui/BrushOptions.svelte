<script lang="ts">
    import { app } from '../state/app.svelte';
    import { brushGraph, exposedDragSpeed } from '../state/brush_graph.svelte';
    import type { BrushInfo } from '../state/brush_graph.svelte';
    import { brushSession } from '../tools/brush.svelte';
    import BrushPicker from './brush_picker/BrushPicker.svelte';
    import LiveBrushPreviewStrip from './brush_picker/LiveBrushPreviewStrip.svelte';

    let brushPickerOpen = $state(false);

    function ensureInit() {
        if (!brushGraph.graph && app.handle) brushGraph.init();
    }

    function toggleBuilder() {
        ensureInit();
        brushGraph.isOpen = !brushGraph.isOpen;
    }

    function selectBrush(brush: BrushInfo) {
        ensureInit();
        brushGraph.loadBrush(brush.name);
        brushPickerOpen = false;
    }

    function handleExposedPort(nodeId: number, portName: string, displayValue: number) {
        brushGraph.setExposedPortValueLocal(nodeId, portName, displayValue);
        brushGraph.setExposedPortValue(nodeId, portName, displayValue);
    }

    /** Format an exposed scalar value based on its unit type. */
    function formatExposedValue(value: number, unitType: string): string {
        switch (unitType) {
            case 'Percent': return `${Math.round(value)}%`;
            case 'Degrees': return `${Math.round(value)}°`;
            case 'Raw': return value.toFixed(2);
            default: return value.toFixed(2); // Normalized
        }
    }

    function handleClickOutside(_e: MouseEvent) {
        if (brushPickerOpen) {
            brushPickerOpen = false;
        }
    }

    function toggleEraseMode() {
        brushSession.eraseMode = !brushSession.eraseMode;
        app.handle?.set_brush_blend_mode(brushSession.eraseMode ? 1 : 0);
    }

    // Brushes whose terminal doesn't honor `gpu.blend_mode` (smudge,
    // liquify, watercolor) report `supportsErase = false`. Reactively
    // force erase-mode off when the user switches to one of them so the
    // session flag and the engine flag don't drift out of sync with the
    // hidden toggle. Re-runs on every graph change because both reads
    // are $state-tracked.
    $effect(() => {
        if (!brushGraph.supportsErase && brushSession.eraseMode) {
            brushSession.eraseMode = false;
            app.handle?.set_brush_blend_mode(0);
        }
    });
</script>

<svelte:window onclick={handleClickOutside} />

<!-- Brush picker -->
<div class="brush-picker-section">
    <button
        class="brush-picker-button"
        onclick={(e) => { e.stopPropagation(); ensureInit(); brushPickerOpen = !brushPickerOpen; }}
        title="Select brush"
    >
        <!-- Live preview of the active graph — same component the
             picker's active strip uses, so preset and custom states
             render identically. The label below is the only thing
             that switches between the preset name and "Custom". -->
        <span class="trigger-preview">
            <LiveBrushPreviewStrip width={80} />
        </span>
        <span class="brush-name">{brushGraph.activeBrush ?? 'Custom'}</span>
        <svg class="chevron" width="10" height="6" viewBox="0 0 10 6">
            <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" fill="none"/>
        </svg>
    </button>

    {#if brushPickerOpen}
        <BrushPicker onSelect={selectBrush} />
    {/if}
</div>

<!-- Scrollable middle region: scrubs + erase toggle. The brush picker
     (left) and the error badge + builder toggle (right) stay pinned
     to the bar; only this strip scrolls horizontally when narrow. -->
<div class="scrub-scroll">
    <!-- Exposed port scrubs from the brush graph -->
    {#each brushGraph.exposedPorts as port}
        {#if port.data.kind === 'scalar'}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="scrub"
                title={port.description || undefined}
                onpointerdown={(e) => {
                    e.preventDefault();
                    const d = port.data as { kind: 'scalar'; value: number; min: number; max: number; default: number; unitType: string };
                    const startX = e.clientX;
                    const startVal = d.value;
                    const speed = exposedDragSpeed(d.min, d.max);
                    const el = e.currentTarget as HTMLElement;
                    el.setPointerCapture(e.pointerId);
                    el.classList.add('dragging');
                    const onMove = (ev: PointerEvent) => {
                        const dx = ev.clientX - startX;
                        const v = Math.min(d.max, Math.max(d.min, startVal + dx * speed));
                        handleExposedPort(port.nodeId, port.portName, v);
                    };
                    const onUp = () => {
                        el.classList.remove('dragging');
                        el.removeEventListener('pointermove', onMove);
                        el.removeEventListener('pointerup', onUp);
                    };
                    el.addEventListener('pointermove', onMove);
                    el.addEventListener('pointerup', onUp);
                }}
                ondblclick={() => {
                    const d = port.data as { kind: 'scalar'; default: number };
                    handleExposedPort(port.nodeId, port.portName, d.default);
                }}
            >
                <i class="{port.icon || 'fa-solid fa-sliders'} scrub-icon"></i>
                <div class="scrub-text">
                    <span class="scrub-label">{port.label}</span>
                    <span class="scrub-value">{formatExposedValue(port.data.value, port.data.unitType)}</span>
                </div>
            </div>
        {/if}
    {/each}

    <!-- Erase-mode toggle. Brush-tool session state lives on the tool
         itself; this toggle just mirrors it and pushes the engine flag.
         Hidden for brushes whose terminal opts out of erase (smudge,
         liquify, watercolor) via `supports_erase = false` on its node
         registration — for those brushes flipping `gpu.blend_mode`
         would do nothing, so the toggle would be a lie. -->
    {#if brushGraph.supportsErase}
        <button
            type="button"
            class="scrub erase-toggle"
            class:on={brushSession.eraseMode}
            onclick={toggleEraseMode}
            title="Erase mode (E)"
        >
            <i class="fa-solid fa-eraser scrub-icon"></i>
            <div class="scrub-text">
                <span class="scrub-label">Erase</span>
                <span class="scrub-value">{brushSession.eraseMode ? 'On' : 'Off'}</span>
            </div>
        </button>
    {/if}
</div>

<!-- Error indicator -->
{#if brushGraph.error}
    <span class="error-badge" title={brushGraph.error}>Error</span>
{/if}

<!-- Bottom bar toggle -->
<button
    class="bottom-bar-toggle"
    onclick={toggleBuilder}
    title={brushGraph.isOpen ? 'Collapse brush builder' : 'Expand brush builder'}
>
    <i class="fa-solid fa-chevron-up" class:flipped={brushGraph.isOpen}></i>
</button>

<style>
    /* ── Scrub Controls ── */

    .scrub {
        flex-shrink: 0;
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 10px;
        border-radius: 6px;
        cursor: col-resize;
        background: var(--bg-hover);
        transition: background 0.1s;
    }

    .scrub:hover {
        background: var(--bg-active);
    }

    .scrub.dragging {
        background: var(--accent);
    }

    :global(.scrub.dragging .scrub-icon),
    :global(.scrub.dragging .scrub-label),
    :global(.scrub.dragging .scrub-value) {
        color: #ffffff;
    }

    /* Erase toggle reuses .scrub layout but is a click-to-toggle
       button, not a drag-to-scrub control. `.on` highlights it when
       erase mode is active — same accent treatment as `.dragging`. */
    .erase-toggle {
        border: none;
        font: inherit;
        cursor: pointer;
    }
    .erase-toggle.on {
        background: var(--accent);
    }
    :global(.erase-toggle.on .scrub-icon),
    :global(.erase-toggle.on .scrub-label),
    :global(.erase-toggle.on .scrub-value) {
        color: #ffffff;
    }

    :global(.scrub-icon) {
        font-size: 14px;
        color: var(--text-muted);
    }

    .scrub-text {
        display: flex;
        flex-direction: column;
    }

    .scrub-label {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        line-height: 1;
    }

    .scrub-value {
        font-size: 12px;
        color: var(--text);
        font-variant-numeric: tabular-nums;
        line-height: 1.3;
    }

    /* ── Brush Picker ── */

    .brush-picker-section {
        position: relative;
        flex-shrink: 0;
    }

    /* Width-bound wrapper for the embedded preview strip — the strip
     * is `width: 100%; aspect-ratio: 11/3`, so the wrapper width
     * picks the trigger preview's height. 80px → ~22px tall. */
    .trigger-preview {
        display: block;
        width: 80px;
        flex-shrink: 0;
    }

    .brush-picker-button {
        display: flex;
        align-items: center;
        gap: 4px;
        /* Gradient lives on the border, not the fill — two-layer
         * background with `padding-box`/`border-box` clips so the
         * border-radius is preserved (border-image flattens corners). */
        background:
            linear-gradient(var(--bg), var(--bg)) padding-box,
            linear-gradient(to right, var(--thumb-bg), var(--bg)) border-box;
        border: 3px solid transparent;
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        font-size: 13px;
        font-weight: 600;
        padding: 2px 6px;
        min-width: 100px;
        transition: filter 0.1s;
    }
    .brush-picker-button:hover {
        filter: brightness(1.15);
    }

    .brush-name {
        flex: 1;
        text-align: left;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .chevron {
        flex-shrink: 0;
        color: var(--text-muted);
    }

    /* ── Scrollable middle & right-side controls ── */

    /* Takes all leftover horizontal space; scrolls when its
     * children don't fit. `min-width: 0` is required for a flex
     * child to be allowed to shrink below its content size — without
     * it the parent would grow and the bar would overflow its column
     * instead of letting this region scroll. */
    .scrub-scroll {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 4px;
        overflow-x: auto;
        overflow-y: hidden;
        scrollbar-width: thin;
    }

    .error-badge {
        font-size: 9px;
        color: var(--danger);
        background: var(--bg-active);
        padding: 2px 6px;
        border-radius: 3px;
        cursor: help;
        flex-shrink: 0;
    }

    .bottom-bar-toggle {
        width: 28px;
        height: 28px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 6px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
        flex-shrink: 0;
        transition: color 0.1s, background 0.1s;
    }

    .bottom-bar-toggle:hover {
        background: var(--bg-active);
        color: var(--text);
    }

    .bottom-bar-toggle i {
        transition: transform 0.2s ease-out;
    }

    .bottom-bar-toggle .flipped {
        transform: rotate(180deg);
    }
</style>
