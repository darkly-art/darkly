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

<div class="brush-bar-wrapper">
    <!-- Brush builder panel (expands above the bar) -->
    {#if brushGraph.isOpen}
        <div class="builder-panel">
            <BrushBuilder />
        </div>
    {/if}

    <!-- Persistent bottom bar -->
    <div class="brush-bar">
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
                <div class="preset-dropdown" onclick={(e) => e.stopPropagation()}>
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

        <div class="separator"></div>

        <!-- Brush size -->
        <div class="slider-control">
            <span class="control-label">Size</span>
            <input
                type="range"
                class="bar-slider"
                min="1" max="500" step="1"
                bind:value={app.brushSize}
            />
            <span class="control-value">{app.brushSize}</span>
        </div>

        <!-- Brush opacity -->
        <div class="slider-control">
            <span class="control-label">Opacity</span>
            <input
                type="range"
                class="bar-slider"
                min="0" max="1" step="0.01"
                bind:value={app.brushOpacity}
            />
            <span class="control-value">{Math.round(app.brushOpacity * 100)}%</span>
        </div>

        <!-- User input sliders from the brush graph -->
        {#each brushGraph.userInputs as input}
            <div class="separator"></div>
            <div class="slider-control">
                <span class="control-label">{input.label}</span>
                <input
                    type="range"
                    class="bar-slider"
                    min="0" max="1" step="0.01"
                    value={input.value}
                    oninput={(e) => handleUserInput(input.nodeId, 1, parseFloat((e.target as HTMLInputElement).value))}
                />
                <span class="control-value">{Math.round(input.value * 100)}%</span>
            </div>
        {/each}

        <div class="spacer"></div>

        <!-- Error indicator -->
        {#if brushGraph.error}
            <span class="error-badge" title={brushGraph.error}>Error</span>
        {/if}

        <!-- Builder toggle -->
        <button
            class="builder-toggle"
            class:active={brushGraph.isOpen}
            onclick={toggleBuilder}
            title={brushGraph.isOpen ? 'Close brush builder' : 'Open brush builder'}
        >
            <svg width="14" height="14" viewBox="0 0 14 14">
                {#if brushGraph.isOpen}
                    <path d="M1 10l6-6 6 6" stroke="currentColor" stroke-width="1.5" fill="none"/>
                {:else}
                    <path d="M1 4l6 6 6-6" stroke="currentColor" stroke-width="1.5" fill="none"/>
                {/if}
            </svg>
        </button>
    </div>
</div>

<style>
    .brush-bar-wrapper {
        display: flex;
        flex-direction: column;
        flex-shrink: 0;
    }

    .builder-panel {
        height: 280px;
        min-height: 200px;
    }

    .brush-bar {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 4px 8px;
        background: #252525;
        border-top: 1px solid #333;
        min-height: 32px;
        position: relative;
    }

    /* Preset selector */
    .preset-section {
        position: relative;
    }

    .preset-button {
        display: flex;
        align-items: center;
        gap: 4px;
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 4px;
        color: #ddd;
        cursor: pointer;
        font-size: 11px;
        padding: 4px 8px;
        min-width: 100px;
    }
    .preset-button:hover {
        background: #333;
        border-color: #555;
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
        color: #888;
    }

    .preset-dropdown {
        position: absolute;
        bottom: 100%;
        left: 0;
        min-width: 180px;
        max-height: 300px;
        overflow-y: auto;
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 4px;
        margin-bottom: 4px;
        padding: 4px 0;
        z-index: 100;
        box-shadow: 0 -4px 12px rgba(0, 0, 0, 0.4);
    }

    .preset-category {
        font-size: 9px;
        color: #888;
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
        color: #ccc;
        cursor: pointer;
        font-size: 11px;
        padding: 4px 12px;
    }
    .preset-item:hover {
        background: #3a3a3a;
    }
    .preset-item.active {
        color: #7a7aff;
        background: #2a2a3a;
    }

    .preset-empty {
        font-size: 11px;
        color: #666;
        padding: 8px 12px;
        font-style: italic;
    }

    /* Separator */
    .separator {
        width: 1px;
        height: 20px;
        background: #444;
        flex-shrink: 0;
    }

    /* Slider controls */
    .slider-control {
        display: flex;
        align-items: center;
        gap: 4px;
    }

    .control-label {
        font-size: 10px;
        color: #888;
        white-space: nowrap;
        min-width: 32px;
    }

    .bar-slider {
        width: 80px;
        height: 4px;
        -webkit-appearance: none;
        appearance: none;
        background: #444;
        border-radius: 2px;
        outline: none;
        cursor: pointer;
    }

    .bar-slider::-webkit-slider-thumb {
        -webkit-appearance: none;
        appearance: none;
        width: 12px;
        height: 12px;
        border-radius: 50%;
        background: #6a6aff;
        cursor: pointer;
    }

    .bar-slider::-moz-range-thumb {
        width: 12px;
        height: 12px;
        border-radius: 50%;
        background: #6a6aff;
        border: none;
        cursor: pointer;
    }

    .control-value {
        font-size: 10px;
        color: #aaa;
        min-width: 28px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .spacer {
        flex: 1;
    }

    .error-badge {
        font-size: 9px;
        color: #ff6b6b;
        background: #3a2020;
        padding: 2px 6px;
        border-radius: 3px;
        cursor: help;
    }

    /* Builder toggle */
    .builder-toggle {
        background: #2a2a2a;
        border: 1px solid transparent;
        border-radius: 4px;
        color: #888;
        cursor: pointer;
        padding: 4px 6px;
        display: flex;
        align-items: center;
    }
    .builder-toggle:hover {
        background: #333;
        color: #ccc;
    }
    .builder-toggle.active {
        background: #3a3a3a;
        border-color: #6a6aff;
        color: #ccc;
    }
</style>
