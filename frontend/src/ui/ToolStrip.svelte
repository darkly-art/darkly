<script lang="ts">
    import { untrack } from 'svelte';
    import { app } from '../state/app.svelte';
    import { toolRegistry, toolClusterRegistry, type ToolDescriptor, type ToolCluster as ToolClusterDef } from '../tools/registry';
    import ToolCluster from './ToolCluster.svelte';
    import Icon from '../icons/Icon.svelte';

    // Track the last-activated sub-tool per cluster id so a cluster-button
    // click can restore the artist's previous choice. The mutation is wrapped
    // in `untrack` so the write doesn't subscribe this effect to its own
    // target; otherwise the spread-and-reassign would re-fire infinitely.
    $effect(() => {
        const id = app.activeToolId;
        const clusterId = toolRegistry.get(id)?.cluster;
        if (!clusterId) return;
        untrack(() => {
            app.lastToolByCluster[clusterId] = id;
        });
    });

    // Build a flat list of toolbar items (individual tool buttons OR cluster
    // flyouts), then split into groups by tool.group for visual separators.
    //
    // A tool that belongs to a cluster is hidden as a standalone button: the
    // cluster takes its slot at the position of its first member in
    // registration order. Subsequent members are skipped.
    type ToolbarItem =
        | { kind: 'tool'; tool: ToolDescriptor; group: string }
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
                // Cluster id is set but not registered; fall through and
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

<div class="tool-rail">
    <div class="toolbar">
        {#each toolbarGroups as group}
            <div class="tool-group">
                {#each group.items as item}
                    {#if item.kind === 'cluster'}
                        <ToolCluster cluster={item.cluster} />
                    {:else}
                        <button
                            class="icon-btn square tool"
                            class:active={app.activeToolId === item.tool.id}
                            onclick={() => app.activeToolId = item.tool.id}
                            title={app.toolTooltip(item.tool.id)}
                        >
                            <Icon name={app.toolGlyph(item.tool.id)} />
                        </button>
                    {/if}
                {/each}
            </div>
        {/each}
    </div>
</div>

<style>
    /* Full-height transparent rail spanning the canvas region's left edge. It
       exists only to position the strip: `safe center` keeps the strip centered
       while it fits and falls back to start alignment when it does not, so a
       short canvas region (a tall brush builder, a tiled panel) makes the strip
       spill downward over the tool-options bar instead of clipping off the top.
       `pointer-events: none` so the empty rail never swallows a canvas press;
       the strip re-enables them for itself.

       The rail must out-rank `CanvasOverlay`'s z-index 1: the WebGPU canvases
       are a fixed, z-indexed layer positioned over this panel's placeholder,
       not children of it. Two constraints follow, and both are load-bearing.
       Nothing between here and the document root may create a stacking context,
       or this z-index stops competing with the overlay's and the strip vanishes
       behind the canvas. And the rail *is* a stacking context itself, so any
       popup raised inside the strip is capped at this level relative to the
       rest of the app; that is why the hamburger menu lives in the tab row and
       the color popup in the tool-options bar, and why ToolCluster's flyout,
       which only needs to beat the canvas, is the only surface left in here. */
    .tool-rail {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        z-index: 2;
        display: flex;
        flex-direction: column;
        justify-content: safe center;
        pointer-events: none;
    }

    /* Flush against the left edge, rounded and shadowed on the open side. The
       same shape ToolCluster's flyout uses, so a strip with an open flyout
       reads as one surface. Deliberately unscrolled: `overflow-y: auto` would
       clip those flyouts, which are positioned at `left: 100%`. */
    .toolbar {
        pointer-events: auto;
        width: 44px;
        display: flex;
        flex-direction: column;
        align-items: center;
        padding: 6px 0;
        gap: 2px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-left: none;
        border-radius: 0 var(--radius-md) var(--radius-md) 0;
        box-shadow: 2px 2px 12px rgba(0, 0, 0, 0.3);
    }

    .tool-group {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding-bottom: 6px;
    }

    .tool-group:last-child {
        padding-bottom: 0;
    }

    .tool-group + .tool-group {
        padding-top: 6px;
        border-top: 1px solid var(--bg-hover);
    }

    /* `fill: currentColor` so SVGs downloaded from icon sets (Font Awesome,
       Boxicons, etc.) inherit the strip's text color exactly like the webfont
       icons do. Without it, raw FA SVG downloads render black because their
       paths have no explicit fill. Descendant paths inherit fill from the
       <svg> element, so per-element fills in fancier SVGs still win. */
    .tool :global(svg) {
        fill: currentColor;
    }

    .tool.active {
        background: var(--accent);
        color: #ffffff;
    }
</style>
