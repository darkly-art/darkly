<script lang="ts">
    import { app } from '../state/app.svelte';
    import { brushGraph } from '../state/brush_graph.svelte';
    import type { BrushInfo } from '../state/brush_graph.svelte';
    import { brushSession } from '../tools/brush.svelte';
    import BrushPicker from './brush_picker/BrushPicker.svelte';
    import LiveBrushPreviewStrip from './brush_picker/LiveBrushPreviewStrip.svelte';
    import Scrub from './Scrub.svelte';
    import ToolBarLayout from './ToolBarLayout.svelte';
    import { tooltipForAction } from '../config/store.svelte';

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
    function formatExposedValue(unitType: string): (v: number) => string {
        switch (unitType) {
            case 'Percent': return (v) => `${Math.round(v)}%`;
            case 'Degrees': return (v) => `${Math.round(v)}°`;
            case 'Raw': return (v) => v.toFixed(2);
            default: return (v) => v.toFixed(2); // Normalized
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

<ToolBarLayout>
    {#snippet left()}
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
    {/snippet}

    {#snippet center()}
        {#each brushGraph.exposedPorts as port}
            {#if port.data.kind === 'scalar'}
                {@const d = port.data}
                <Scrub
                    mode="drag"
                    icon={port.icon || undefined}
                    label={port.label}
                    value={d.value}
                    min={d.min}
                    max={d.max}
                    default={d.default}
                    formatValue={formatExposedValue(d.unitType)}
                    onChange={(v) => handleExposedPort(port.nodeId, port.portName, v)}
                    title={port.description || undefined}
                />
            {/if}
        {/each}

        <!-- Erase-mode toggle. Brush-tool session state lives on the tool
             itself; this toggle just mirrors it and pushes the engine flag.
             Hidden for brushes whose terminal opts out of erase (smudge,
             liquify, watercolor) via `supports_erase = false` on its node
             registration — for those brushes flipping `gpu.blend_mode`
             would do nothing, so the toggle would be a lie. -->
        {#if brushGraph.supportsErase}
            <Scrub
                mode="toggle"
                icon="fa-solid fa-eraser"
                label="Erase"
                valueLabel={brushSession.eraseMode ? 'On' : 'Off'}
                active={brushSession.eraseMode}
                onToggle={toggleEraseMode}
                title={tooltipForAction('Erase mode', 'toggleEraseMode')}
            />
        {/if}
    {/snippet}

    {#snippet right()}
        {#if brushGraph.error}
            <span class="error-badge" title={brushGraph.error}>Error</span>
        {/if}

        <button
            class="bottom-bar-toggle"
            onclick={toggleBuilder}
            title={brushGraph.isOpen ? 'Collapse brush builder' : 'Expand brush builder'}
        >
            <i class="fa-solid fa-chevron-up" class:flipped={brushGraph.isOpen}></i>
        </button>
    {/snippet}
</ToolBarLayout>

<style>
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
