<script lang="ts">
    import { app } from '../state/app.svelte';
    import { toolRegistry } from '../tools/registry';
    import ColorPicker from './ColorPicker.svelte';

    let showColorPicker = $state(false);

    function colorStyle(c: { r: number; g: number; b: number; a: number }): string {
        return `rgb(${c.r}, ${c.g}, ${c.b})`;
    }

    function toggleColorPicker() {
        showColorPicker = !showColorPicker;
    }
</script>

<div class="left-sidebar">
    <!-- Color swatches -->
    <div class="color-section">
        <button class="color-swatches" onclick={toggleColorPicker} title="Pick color">
            <div
                class="swatch bg-swatch"
                style="background: {colorStyle(app.background)}"
            ></div>
            <div
                class="swatch fg-swatch"
                style="background: {colorStyle(app.foreground)}"
            ></div>
        </button>
        <button class="swap-btn" onclick={() => app.swapColors()} title="Swap colors (X)">
            &#x21C4;
        </button>
    </div>

    {#if showColorPicker}
        <ColorPicker onclose={() => showColorPicker = false} />
    {/if}

    <!-- Tool buttons -->
    <div class="tool-buttons">
        {#each toolRegistry.all() as tool}
            <button
                class="tool-btn"
                class:active={app.activeToolId === tool.id}
                onclick={() => app.activeToolId = tool.id}
                title="{tool.name} ({tool.icon})"
            >
                {tool.icon}
            </button>
        {/each}
    </div>

    <!-- Brush size display -->
    <div class="info-display">
        <span class="info-label">Size</span>
        <span class="info-value">{app.brushSize}</span>
    </div>
    <div class="info-display">
        <span class="info-label">Opacity</span>
        <span class="info-value">{Math.round(app.brushOpacity * 100)}%</span>
    </div>
</div>

<style>
    .left-sidebar {
        width: 48px;
        min-width: 48px;
        background: #222;
        border-right: 1px solid #333;
        display: flex;
        flex-direction: column;
        align-items: center;
        padding: 8px 0;
        gap: 4px;
        position: relative;
    }

    .color-section {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 2px;
        margin-bottom: 8px;
    }

    .color-swatches {
        position: relative;
        width: 32px;
        height: 32px;
        cursor: pointer;
        background: none;
        border: none;
        padding: 0;
    }

    .swatch {
        position: absolute;
        border: 1px solid #555;
        border-radius: 2px;
    }

    .fg-swatch {
        width: 20px;
        height: 20px;
        top: 0;
        left: 0;
        z-index: 1;
    }

    .bg-swatch {
        width: 20px;
        height: 20px;
        bottom: 0;
        right: 0;
    }

    .swap-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        font-size: 10px;
        padding: 0;
        line-height: 1;
    }
    .swap-btn:hover { color: #ccc; }

    .tool-buttons {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .tool-btn {
        width: 36px;
        height: 36px;
        background: #2a2a2a;
        border: 1px solid transparent;
        border-radius: 4px;
        color: #ccc;
        cursor: pointer;
        font-size: 14px;
        font-weight: 600;
        display: flex;
        align-items: center;
        justify-content: center;
    }

    .tool-btn:hover {
        background: #333;
    }

    .tool-btn.active {
        background: #3a3a3a;
        border-color: #6a6aff;
    }

    .info-display {
        display: flex;
        flex-direction: column;
        align-items: center;
        font-size: 9px;
        color: #888;
        margin-top: 4px;
    }

    .info-label {
        font-size: 8px;
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }

    .info-value {
        color: #ccc;
        font-size: 10px;
    }
</style>
