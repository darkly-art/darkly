<script lang="ts">
    import { app } from '../state/app.svelte';
    import { toolRegistry } from '../tools/registry';
    import { config, formatHotkey } from '../config/store.svelte';
    import ColorPicker from './ColorPicker.svelte';
    import HamburgerMenu from './HamburgerMenu.svelte';

    let showColorPicker = $state(false);

    function colorStyle(c: { r: number; g: number; b: number; a: number }): string {
        return `rgb(${c.r}, ${c.g}, ${c.b})`;
    }

    function toggleColorPicker() {
        showColorPicker = !showColorPicker;
    }

    // Group tools by their group property for visual separation
    interface ToolGroup { tools: ReturnType<typeof toolRegistry.all> }
    let toolGroups = $derived((() => {
        const all = toolRegistry.all();
        const groups: ToolGroup[] = [];
        let current: ReturnType<typeof toolRegistry.all> = [];
        let currentGroup: string | undefined = undefined;
        for (const t of all) {
            const g = t.group ?? '';
            if (g !== currentGroup && current.length > 0) {
                groups.push({ tools: current });
                current = [];
            }
            currentGroup = g;
            current.push(t);
        }
        if (current.length > 0) groups.push({ tools: current });
        return groups;
    })());
</script>

<div class="toolbar">
    <HamburgerMenu />

    <div class="toolbar-spacer"></div>

    <!-- Tool buttons (vertically centered) -->
    {#each toolGroups as group, i}
        <div class="tool-group">
            {#each group.tools as tool}
                <button
                    class="tool"
                    class:active={app.activeToolId === tool.id}
                    onclick={() => app.activeToolId = tool.id}
                    title={(() => { const hk = formatHotkey(config.get(`hotkeys.${tool.hotkeyAction}`)); return hk ? `${tool.name} (${hk})` : tool.name; })()}
                >
                    <i class={tool.faIcon}></i>
                </button>
            {/each}
        </div>
    {/each}

    <div class="toolbar-spacer"></div>

    <!-- Color swatches + swap (bottom) -->
    <div class="toolbar-bottom">
        <div class="color-swatches">
            <button class="swatch-stack" onclick={toggleColorPicker} title="Pick color">
                <div
                    class="swatch bg"
                    style="background: {colorStyle(app.background)}"
                ></div>
                <div
                    class="swatch fg"
                    style="background: {colorStyle(app.foreground)}"
                ></div>
            </button>
        </div>
        <button class="tool swap" onclick={() => app.swapColors()} title={(() => { const hk = formatHotkey(config.get('hotkeys.swapColors')); return hk ? `Swap colors (${hk})` : 'Swap colors'; })()}>
            <i class="fa-solid fa-arrow-right-arrow-left"></i>
        </button>
    </div>

    {#if showColorPicker}
        <ColorPicker onclose={() => showColorPicker = false} />
    {/if}
</div>

<style>
    .toolbar {
        width: 44px;
        background: var(--bg);
        display: flex;
        flex-direction: column;
        align-items: center;
        padding: 6px 0;
        gap: 2px;
        flex-shrink: 0;
    }

    .color-swatches {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 4px;
    }

    .swatch-stack {
        position: relative;
        width: 28px;
        height: 28px;
        cursor: pointer;
        background: none;
        border: none;
        padding: 0;
    }

    .swatch {
        position: absolute;
        border-radius: 4px;
        cursor: pointer;
    }

    .swatch.fg {
        width: 20px;
        height: 20px;
        top: 0;
        left: 0;
        z-index: 1;
        box-shadow: 0 0 0 1px var(--text-dim);
    }

    .swatch.bg {
        width: 20px;
        height: 20px;
        bottom: 0;
        right: 0;
        box-shadow: 0 0 0 1px var(--text-dim);
    }

    .tool-group {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding-bottom: 6px;
    }

    .tool-group + .tool-group {
        padding-top: 6px;
        border-top: 1px solid var(--bg-hover);
    }

    .tool {
        width: 32px;
        height: 32px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 6px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 14px;
        transition: background 0.1s, color 0.1s;
    }

    .tool:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .tool.active {
        background: var(--accent);
        color: #ffffff;
    }

    .tool.swap {
        width: 28px;
        height: 20px;
        font-size: 10px;
    }

    .toolbar-spacer {
        flex: 1;
    }

    .toolbar-bottom {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 6px;
        padding-top: 6px;
        border-top: 1px solid var(--bg-hover);
    }
</style>
