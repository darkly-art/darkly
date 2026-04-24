<script lang="ts">
    import { app } from '../state/app.svelte';
    import { brushGraph } from '../state/brush_graph.svelte';
    import type { PresetInfo } from '../state/brush_graph.svelte';
    import BrushBuilder from './brush_builder/BrushBuilder.svelte';

    let presetDropdownOpen = $state(false);

    function ensureInit() {
        if (!brushGraph.graph && app.handle) brushGraph.init();
    }

    function toggleBuilder() {
        ensureInit();
        brushGraph.isOpen = !brushGraph.isOpen;
    }

    function selectPreset(preset: PresetInfo) {
        ensureInit();
        brushGraph.loadPreset(preset.name);
        presetDropdownOpen = false;
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

    /** Drag speed scaled to the display range. */
    function exposedDragSpeed(min: number, max: number): number {
        const range = max - min;
        return range / 400; // ~400px of drag to cover the full range
    }

    function handleClickOutside(e: MouseEvent) {
        if (presetDropdownOpen) {
            presetDropdownOpen = false;
        }
    }

    let builderHeight = $state(33); // vh units

    function handleResizeStart(e: PointerEvent) {
        e.preventDefault();
        const el = e.currentTarget as HTMLElement;
        el.setPointerCapture(e.pointerId);
        const startY = e.clientY;
        const startHeight = builderHeight;
        const vh = window.innerHeight / 100;

        const onMove = (ev: PointerEvent) => {
            const dy = startY - ev.clientY; // dragging up = increase height
            builderHeight = Math.min(80, Math.max(15, startHeight + dy / vh));
        };
        const onUp = () => {
            el.removeEventListener('pointermove', onMove);
            el.removeEventListener('pointerup', onUp);
        };
        el.addEventListener('pointermove', onMove);
        el.addEventListener('pointerup', onUp);
    }

    // Group presets by category
    function groupedPresets(): Map<string, PresetInfo[]> {
        const groups = new Map<string, PresetInfo[]>();
        for (const p of brushGraph.presets) {
            const cat = p.category || 'uncategorized';
            if (!groups.has(cat)) groups.set(cat, []);
            groups.get(cat)!.push(p);
        }
        return groups;
    }

</script>

<svelte:window onclick={handleClickOutside} />

<div class="bottom-area">
    <!-- Expandable brush builder -->
    {#if brushGraph.isOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="resize-handle" onpointerdown={handleResizeStart}></div>
        <div class="builder-panel" style="height: {builderHeight}vh">
            <BrushBuilder />
        </div>
    {/if}

    <!-- Tool options bar (always visible) -->
    <div class="tool-options">
        <!-- Preset selector -->
        <div class="preset-section">
            <button
                class="preset-button"
                onclick={(e) => { e.stopPropagation(); ensureInit(); presetDropdownOpen = !presetDropdownOpen; }}
                title="Select brush preset"
            >
                <span class="preset-name">{brushGraph.activePreset ?? 'Custom'}</span>
                <svg class="chevron" width="10" height="6" viewBox="0 0 10 6">
                    <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" fill="none"/>
                </svg>
            </button>

            {#if presetDropdownOpen}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <div class="preset-dropdown dropdown-surface" onclick={(e) => e.stopPropagation()}>
                    {#each [...groupedPresets()] as [category, presets]}
                        <div class="preset-category">{category}</div>
                        {#each presets as preset}
                            <button
                                class="preset-item"
                                class:active={brushGraph.activePreset === preset.name}
                                onclick={() => selectPreset(preset)}
                            >
                                {preset.name}
                            </button>
                        {/each}
                    {/each}
                    {#if brushGraph.presets.length === 0}
                        <div class="preset-empty">No presets available</div>
                    {/if}
                </div>
            {/if}
        </div>

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

        <div class="spacer"></div>

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
    </div>
</div>

<style>
    .bottom-area {
        display: flex;
        flex-direction: column;
        flex-shrink: 0;
    }

    /* ── Tool Options Bar ── */

    .tool-options {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 4px;
        padding: 4px 8px;
        background: var(--bg);
        flex-shrink: 0;
    }

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

    /* ── Preset Selector ── */

    .preset-section {
        position: relative;
        flex-shrink: 0;
    }

    .preset-button {
        display: flex;
        align-items: center;
        gap: 4px;
        background: var(--bg-hover);
        border: none;
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        font-size: 11px;
        padding: 4px 8px;
        min-width: 100px;
        transition: background 0.1s;
    }
    .preset-button:hover {
        background: var(--bg-active);
    }

    .preset-name {
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

    .preset-dropdown {
        position: absolute;
        bottom: 100%;
        left: 0;
        min-width: 180px;
        max-height: 300px;
        overflow-y: auto;
        margin-bottom: 4px;
        padding: 4px 0;
        z-index: 100;
    }

    .preset-category {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        padding: 6px 12px 2px;
    }

    .preset-item {
        display: block;
        width: 100%;
        text-align: left;
        background: none;
        border: none;
        color: var(--text);
        cursor: pointer;
        font-size: 11px;
        padding: 4px 12px;
    }
    .preset-item:hover {
        background: var(--bg-hover);
    }
    .preset-item.active {
        color: var(--accent);
    }

    .preset-empty {
        font-size: 11px;
        color: var(--text-dim);
        padding: 8px 12px;
        font-style: italic;
    }

    /* ── Spacer & Toggle ── */

    .spacer {
        flex: 1;
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

    /* ── Builder Panel ── */

    .resize-handle {
        height: 5px;
        cursor: ns-resize;
        background: transparent;
        flex-shrink: 0;
        transition: background 0.1s;
    }
    .resize-handle:hover,
    .resize-handle:active {
        background: var(--accent);
    }

    .builder-panel {
        min-height: 100px;
        border-bottom: 1px solid var(--bg-hover);
    }
</style>
