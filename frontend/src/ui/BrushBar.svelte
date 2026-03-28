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

    function handleUserInput(nodeId: number, paramIndex: number, value: number) {
        brushGraph.setParamLocal(nodeId, paramIndex, value);
        brushGraph.setParam(nodeId, paramIndex, 'float', value);
    }

    /** Format a user input value based on its units enum. */
    function formatUserInput(value: number, units: number): string {
        switch (units) {
            case 1: return `${Math.round(value)} px`;
            case 2: return `${Math.round(value)}°`;
            case 3: return value.toFixed(2);
            default: return `${Math.round(value * 100)}%`;
        }
    }

    /** Drag speed scaled to the input's range. */
    function userInputDragSpeed(min: number, max: number, units: number): number {
        const range = max - min;
        if (units === 0) return 0.005; // percent: 0-1 range, slow drag
        return range / 400; // ~400px of drag to cover the full range
    }

    function handleClickOutside(e: MouseEvent) {
        if (presetDropdownOpen) {
            presetDropdownOpen = false;
        }
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

        <!-- User input scrubs from the brush graph -->
        {#each brushGraph.userInputs as input}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="scrub"
                title={input.description || undefined}
                onpointerdown={(e) => {
                    e.preventDefault();
                    const startX = e.clientX;
                    const startVal = input.value;
                    const speed = userInputDragSpeed(input.min, input.max, input.units);
                    const el = e.currentTarget as HTMLElement;
                    el.setPointerCapture(e.pointerId);
                    el.classList.add('dragging');
                    const onMove = (ev: PointerEvent) => {
                        const dx = ev.clientX - startX;
                        const v = Math.min(input.max, Math.max(input.min, startVal + dx * speed));
                        handleUserInput(input.nodeId, 1, v);
                    };
                    const onUp = () => {
                        el.classList.remove('dragging');
                        el.removeEventListener('pointermove', onMove);
                        el.removeEventListener('pointerup', onUp);
                    };
                    el.addEventListener('pointermove', onMove);
                    el.addEventListener('pointerup', onUp);
                }}
            >
                <i class="{input.icon || 'fa-solid fa-sliders'} scrub-icon"></i>
                <div class="scrub-text">
                    <span class="scrub-label">{input.label}</span>
                    <span class="scrub-value">{formatUserInput(input.value, input.units)}</span>
                </div>
            </div>
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

    <!-- Expandable brush builder -->
    {#if brushGraph.isOpen}
        <div class="builder-panel">
            <BrushBuilder />
        </div>
    {/if}
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
        background: var(--bg-raised);
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

    .builder-panel {
        height: 33vh;
        min-height: 200px;
        border-top: 1px solid var(--bg-hover);
    }
</style>
