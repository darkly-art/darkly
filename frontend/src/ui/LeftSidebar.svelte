<script lang="ts">
    import { untrack } from 'svelte';
    import { app } from '../state/app.svelte';
    import { toolRegistry, toolClusterRegistry, type Tool, type ToolCluster as ToolClusterDef } from '../tools/registry';
    import { config, formatHotkey } from '../config/store.svelte';
    import ColorPicker from './ColorPicker.svelte';
    import HamburgerMenu from './HamburgerMenu.svelte';
    import ToolCluster from './ToolCluster.svelte';

    let showColorPicker = $state(false);
    let pickerEl: HTMLDivElement | undefined = $state();
    let swatchEl: HTMLButtonElement | undefined = $state();

    // Track the last-activated sub-tool per cluster id so a cluster-button
    // click can restore the user's previous choice. The mutation is wrapped
    // in `untrack` so the write doesn't subscribe this effect to its own
    // target — otherwise the spread-and-reassign would re-fire infinitely.
    $effect(() => {
        const id = app.activeToolId;
        const clusterId = toolRegistry.get(id)?.cluster;
        if (!clusterId) return;
        untrack(() => {
            app.lastToolByCluster[clusterId] = id;
        });
    });

    function colorStyle(c: { r: number; g: number; b: number; a: number }): string {
        return `rgb(${c.r}, ${c.g}, ${c.b})`;
    }

    function toggleColorPicker() {
        showColorPicker = !showColorPicker;
    }

    $effect(() => {
        if (!showColorPicker) return;
        const onPointerDown = (e: PointerEvent) => {
            const t = e.target as Node | null;
            if (!t) return;
            if (pickerEl?.contains(t)) return;
            if (swatchEl?.contains(t)) return;
            showColorPicker = false;
        };
        window.addEventListener('pointerdown', onPointerDown, true);
        return () => window.removeEventListener('pointerdown', onPointerDown, true);
    });

    // Build a flat list of toolbar items (individual tool buttons OR cluster
    // flyouts), then split into groups by tool.group for visual separators.
    //
    // A tool that belongs to a cluster is hidden as a standalone button — the
    // cluster takes its slot at the position of its first member in
    // registration order. Subsequent members are skipped.
    type ToolbarItem =
        | { kind: 'tool'; tool: Tool; group: string }
        | { kind: 'cluster'; cluster: ToolClusterDef; group: string };
    interface ToolbarGroup { items: ToolbarItem[] }

    let toolbarGroups = $derived((() => {
        const items: ToolbarItem[] = [];
        const placedClusters = new Set<string>();
        for (const t of toolRegistry.all()) {
            if (t.cluster) {
                if (placedClusters.has(t.cluster)) continue;
                const cluster = toolClusterRegistry.get(t.cluster);
                if (cluster) {
                    placedClusters.add(t.cluster);
                    items.push({ kind: 'cluster', cluster, group: t.group ?? '' });
                    continue;
                }
                // Cluster id is set but not registered — fall through and
                // render the tool as a standalone button so it isn't lost.
            }
            items.push({ kind: 'tool', tool: t, group: t.group ?? '' });
        }

        const groups: ToolbarGroup[] = [];
        let current: ToolbarItem[] = [];
        let currentGroup: string | undefined = undefined;
        for (const it of items) {
            if (it.group !== currentGroup && current.length > 0) {
                groups.push({ items: current });
                current = [];
            }
            currentGroup = it.group;
            current.push(it);
        }
        if (current.length > 0) groups.push({ items: current });
        return groups;
    })());
</script>

<div class="toolbar">
    <HamburgerMenu />

    <div class="toolbar-spacer"></div>

    <!-- Tool buttons (vertically centered) -->
    {#each toolbarGroups as group}
        <div class="tool-group">
            {#each group.items as item}
                {#if item.kind === 'cluster'}
                    <ToolCluster cluster={item.cluster} />
                {:else}
                    <button
                        class="tool"
                        class:active={app.activeToolId === item.tool.id}
                        onclick={() => app.activeToolId = item.tool.id}
                        title={(() => { const hk = formatHotkey(config.get(`hotkeys.${item.tool.hotkeyAction}`) as string | undefined); const name = app.toolDisplayName(item.tool.id); return hk ? `${name} (${hk})` : name; })()}
                    >
                        {#if item.tool.iconSvg}
                            {@html item.tool.iconSvg}
                        {:else}
                            <i class={item.tool.faIcon}></i>
                        {/if}
                    </button>
                {/if}
            {/each}
        </div>
    {/each}

    <div class="toolbar-spacer"></div>

    <!-- Color swatches + swap (bottom) -->
    <div class="toolbar-bottom">
        <div class="color-swatches">
            <button bind:this={swatchEl} class="swatch-stack" onclick={toggleColorPicker} title="Pick color">
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
        <button class="tool swap" onclick={() => app.swapColors()} title={(() => { const hk = formatHotkey(config.get('hotkeys.swapColors') as string | undefined); return hk ? `Swap colors (${hk})` : 'Swap colors'; })()}>
            <i class="fa-solid fa-arrow-right-arrow-left"></i>
        </button>
    </div>

    {#if showColorPicker}
        <div bind:this={pickerEl} class="color-picker-wrapper">
            <ColorPicker onclose={() => showColorPicker = false} />
        </div>
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

    /* Normalize inline SVG icons. Forces 1em sizing regardless of the
       source <svg>'s width/height attributes, and sets `fill: currentColor`
       so SVGs downloaded from icon sets (Font Awesome, Boxicons, etc.)
       inherit the toolbar's text color exactly like the webfont icons do.
       Without this, raw FA SVG downloads default to black because their
       paths have no explicit fill. Descendant paths inherit fill from
       the <svg> element, so per-element fills in fancier SVGs still win. */
    .tool :global(svg) {
        width: 1em;
        height: 1em;
        fill: currentColor;
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

    .color-picker-wrapper {
        display: contents;
    }
</style>
