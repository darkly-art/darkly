<script lang="ts">
    import { untrack } from 'svelte';
    import { app } from '../state/app.svelte';
    import { toolRegistry, toolClusterRegistry, type ToolDescriptor, type ToolCluster as ToolClusterDef } from '../tools/registry';
    import FgBgSwatches from './color/FgBgSwatches.svelte';
    import HamburgerMenu from './HamburgerMenu.svelte';
    import ToolCluster from './ToolCluster.svelte';
    import Icon from '../icons/Icon.svelte';
    import { menuBar } from '../state/menuBar.svelte';

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

<div class="toolbar">
    {#if !menuBar.pinned}
        <HamburgerMenu />
    {/if}

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
                        title={app.toolTooltip(item.tool.id)}
                    >
                        <Icon name={app.toolGlyph(item.tool.id)} />
                    </button>
                {/if}
            {/each}
        </div>
    {/each}

    <div class="toolbar-spacer"></div>

    <div class="toolbar-bottom">
        <FgBgSwatches mode="popup" />
    </div>
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
