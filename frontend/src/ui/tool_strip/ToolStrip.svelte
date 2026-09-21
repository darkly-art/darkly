<script lang="ts">
    import { setContext, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { toolRegistry, toolClusterRegistry, type ToolDescriptor, type ToolCluster as ToolClusterDef } from '../../tools/registry';
    import { canvasSlot } from '../../multi_tab/canvasSlot.svelte';
    import ToolCluster from './ToolCluster.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { toolStripPlacement as placement } from './placement.svelte';
    import { isRow, peekOut, regionLength, stripBox, stripPos } from './geometry';
    import { TOOL_STRIP_OUT } from './context';

    /** Slide out within this distance of the strip's untucked box, tuck back
     *  only past the far one. Chosen, not measured. */
    const NEAR_PX = 32;
    const FAR_PX = 56;

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

    let stripEl = $state<HTMLDivElement | null>(null);

    // The strip's own extent along its free axis, needed to work out how much
    // travel it has inside the region. Measured rather than computed: it is the
    // sum of however many tools are registered, their group separators and the
    // padding, and the registry is populated asynchronously.
    let stripLen = $state(0);
    /** And its extent across that axis: 44px, but measured for the same reason. */
    let stripThickness = $state(0);

    $effect(() => {
        const el = stripEl;
        if (!el) return;
        // Guarded so the jsdom component tests need no stub, matching
        // `ui/brush_explorer/BrushExplorer.svelte`.
        if (typeof ResizeObserver === 'undefined') return;
        const measure = () => {
            const row = isRow(placement.edge);
            stripLen = row ? el.offsetWidth : el.offsetHeight;
            stripThickness = row ? el.offsetHeight : el.offsetWidth;
        };
        measure();
        const ro = new ResizeObserver(measure);
        ro.observe(el);
        return () => ro.disconnect();
    });

    // Position along the docked edge. `canvasSlot` owns the region's rect: the
    // WebGPU canvases are positioned from the same measurement, and one
    // `getBoundingClientRect` reader is the rule (docs/coordinate-systems.md).
    let pos = $derived.by(() => {
        const region = canvasSlot.rect;
        if (!region) return 0;
        return stripPos(placement.offset, regionLength(placement.edge, region), stripLen);
    });

    /** Slid out, either by proximity, by a tap, or pinned by the pref. */
    let out = $state(false);
    /** A tap on a touch device latches the strip out; only a press elsewhere
     *  clears it, because there is no hover to move away. */
    let latched = $state(false);

    let pinned = $derived(!placement.autoHide);
    let showing = $derived(pinned || latched || out);

    // The only thing that crosses from the strip to its clusters, and it carries
    // no direction: a flyout left open when the strip tucks would hang in space
    // over the canvas.
    setContext(TOOL_STRIP_OUT, { get out() { return showing; } });

    /** The strip's box where it would be if untucked. Untucked on purpose: the
     *  trigger zone must not move as the strip slides, or approaching it would
     *  chase itself. */
    let untuckedBox = $derived.by(() => {
        const region = canvasSlot.rect;
        if (!region) return null;
        return stripBox(placement.edge, region, stripLen, stripThickness, pos);
    });

    // Proximity. A listener is the only way: pointer events over the canvas are
    // dispatched to the `<canvas>` inside `CanvasOverlay`, a fixed sibling at
    // the app root, so nothing bubbles through this element's ancestry, and it
    // can never match `:hover` from over there either.
    $effect(() => {
        if (pinned) return;
        // The fullscreen brush builder covers the strip at every dock, so
        // tracking behind it is wasted work.
        if (brushGraph.fullscreen) return;

        const onMove = (e: PointerEvent) => {
            // Any held button means a stroke, a pan, a chord drag or a slider
            // scrub is in flight. `e.buttons` catches all of them with one term
            // and no app-state coupling; `app.pointerActive` misses navigation
            // and drag-bound chords, which return early before setting it.
            if (e.buttons !== 0) return;
            const box = untrack(() => untuckedBox);
            if (!box) return;
            out = peekOut(untrack(() => out), e.clientX, e.clientY, box, NEAR_PX, FAR_PX);
        };
        // No further moves arrive once the pointer leaves the window, so
        // without these the strip stays out indefinitely.
        const onLeave = (e: PointerEvent) => {
            if (!e.relatedTarget) out = false;
        };
        const onBlur = () => { out = false; };

        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerout', onLeave);
        window.addEventListener('blur', onBlur);
        return () => {
            window.removeEventListener('pointermove', onMove);
            window.removeEventListener('pointerout', onLeave);
            window.removeEventListener('blur', onBlur);
        };
    });

    // Touch has no hover, and every move it produces carries a held button, so
    // proximity can never fire there. A tap on the tucked sliver opens the strip
    // instead, and it stays open until a press lands outside: the same
    // dismissal shape ToolCluster uses for a pinned flyout.
    function onStripPointerDown() {
        if (!showing) latched = true;
    }

    $effect(() => {
        if (!latched) return;
        const onPointerDown = (e: PointerEvent) => {
            const t = e.target as Node | null;
            if (t && stripEl?.contains(t)) return;
            latched = false;
        };
        window.addEventListener('pointerdown', onPointerDown, true);
        return () => window.removeEventListener('pointerdown', onPointerDown, true);
    });
</script>

<!-- Geometry lives in `styles/tool-strip-dock.css`, keyed off `data-edge`. This
     component writes the edge and the position and knows nothing else about
     which way it is facing; neither does ToolCluster. -->
<div
    class="toolbar"
    class:out={showing}
    role="toolbar"
    tabindex="-1"
    aria-label="Tools"
    aria-orientation={isRow(placement.edge) ? 'horizontal' : 'vertical'}
    bind:this={stripEl}
    data-edge={placement.edge}
    style:--strip-pos="{pos}px"
    onpointerdown={onStripPointerDown}
>
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

<style>
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
