<script lang="ts">
    import { app } from '../state/app.svelte';
    import { brushGraph } from '../state/brush_graph.svelte';
    import type { BrushInfo } from '../state/brush_graph.svelte';
    import { brushSession } from '../tools/brush.svelte';
    import BrushPicker from './brush_picker/BrushPicker.svelte';
    import LiveBrushPreviewStrip from './brush_picker/LiveBrushPreviewStrip.svelte';
    import Scrub from './Scrub.svelte';
    import ToolBarLayout from './ToolBarLayout.svelte';
    import Icon from '../icons/Icon.svelte';
    import { tooltipForAction } from '../config/store.svelte';
    import { watchDismiss } from '../lib/dismiss';

    let brushPickerOpen = $state(false);

    function ensureInit() {
        if (!brushGraph.graph && app.engine) brushGraph.init();
    }

    function toggleBuilder() {
        ensureInit();
        brushGraph.isOpen = !brushGraph.isOpen;
        // Leaving the builder also leaves fullscreen — otherwise reopening
        // would silently spring back to a window-filling panel.
        if (!brushGraph.isOpen) brushGraph.fullscreen = false;
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

    /** Flip a Bool exposed port — toggles the port's f32 default between 0 and 1
     *  via the standard scalar setter, since Bool is encoded as `default >= 0.5`. */
    function handleExposedBool(nodeId: number, portName: string, current: boolean) {
        const next = current ? 0 : 1;
        brushGraph.setPortDefaultLocal(nodeId, portName, next);
        brushGraph.setExposedPortValue(nodeId, portName, next);
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

    // A pointerdown outside the brush picker (trigger + panel, both tagged
    // data-keep-open="brush-picker") closes it.
    $effect(() => watchDismiss('brush-picker', () => (brushPickerOpen = false)));

    function toggleEraseMode() {
        brushSession.eraseMode = !brushSession.eraseMode;
        app.engine?.api.setBrushBlendMode({ mode: brushSession.eraseMode ? 1 : 0 });
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
            app.engine?.api.setBrushBlendMode({ mode: 0 });
        }
    });
</script>

<ToolBarLayout>
    {#snippet center()}
        <!-- The brush picker is the leading control in the same wrapping row
             as the scrubs — a black rounded button that wraps alongside them.
             Its dropdown menu anchors to this button. -->
        <div class="brush-picker-section">
            <button
                class="brush-picker-button bar-control"
                data-keep-open="brush-picker"
                onclick={() => { ensureInit(); brushPickerOpen = !brushPickerOpen; }}
                title="Select brush"
            >
                <!-- Live preview of the active graph — same component the
                     picker's tiles use, so preset and custom states render
                     identically. The value switches between the preset name
                     and "Custom". The preview stands in for a scrub's icon. -->
                <span class="trigger-preview">
                    <LiveBrushPreviewStrip width={64} />
                </span>
                <span class="bar-control-text">
                    <span class="bar-control-label">Brush</span>
                    <span class="bar-control-value name">{brushGraph.activeBrush ?? 'Custom'}</span>
                </span>
                <svg class="chevron" class:flipped={brushPickerOpen} width="10" height="6" viewBox="0 0 10 6">
                    <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" fill="none"/>
                </svg>
            </button>

            {#if brushPickerOpen}
                <BrushPicker onSelect={selectBrush} onClose={() => (brushPickerOpen = false)} />
            {/if}
        </div>

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
            {:else if port.data.kind === 'bool'}
                {@const d = port.data}
                <Scrub
                    mode="toggle"
                    icon={port.icon || undefined}
                    label={port.label}
                    valueLabel={d.value ? 'On' : 'Off'}
                    active={d.value}
                    onToggle={() => handleExposedBool(port.nodeId, port.portName, d.value)}
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
                icon="fa6-solid:eraser"
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
            <Icon name="fa6-solid:chevron-up" class={brushGraph.isOpen ? 'flipped' : ''} />
        </button>
    {/snippet}
</ToolBarLayout>

<style>
    /* Anchor for the dropdown menu; the button itself sizes to content so it
     * wraps in the scrub row like any other control. */
    .brush-picker-section {
        position: relative;
        flex-shrink: 0;
    }

    /* Width-bound wrapper for the embedded preview strip — the strip
     * is `width: 100%; aspect-ratio: 11/3`, so the wrapper width picks the
     * trigger preview's height. 64px → ~17px tall, matching the scrubs. */
    .trigger-preview {
        display: block;
        width: 64px;
        flex-shrink: 0;
    }

    /* Shared `.bar-control` supplies the look (fill, radius, padding, gap,
     * label/value metrics) so this matches the scrubs; only the button reset
     * and the name's truncation are picker-specific. */
    .brush-picker-button {
        border: none;
        cursor: pointer;
    }
    .brush-picker-button .name {
        max-width: 120px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .chevron {
        flex-shrink: 0;
        color: var(--text-muted);
        transition: transform 0.2s ease-out;
    }
    .chevron.flipped {
        transform: rotate(180deg);
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

    .bottom-bar-toggle :global(svg) {
        transition: transform 0.2s ease-out;
    }

    .bottom-bar-toggle :global(.flipped) {
        transform: rotate(180deg);
    }
</style>
