<script lang="ts">
    import { getContext } from 'svelte';
    import { brushGraph, type NodeInstance, type PortDef } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
    import PortWidget from './PortWidget.svelte';
    import NodePreview from './NodePreview.svelte';
    import type { NodeCanvasContext } from './NodeCanvas.svelte';

    interface Props {
        node: NodeInstance;
    }

    let { node }: Props = $props();

    const { coords } = getContext<NodeCanvasContext>('node-canvas');

    let isSelected = $derived(brushGraph.selectedNode === node.id);
    let outputPorts = $derived(node.ports.filter(p => p.dir === 'Output'));
    let position = $derived(brushGraph.nodePositions[node.id] ?? [0, 0]);
    /** Nodes opt in to an in-card preview thumbnail by type id. The
     *  engine's `brush_node_preview` matches on the same id and returns
     *  PNG bytes (or an empty Vec, which the NodePreview component
     *  treats as "no preview"). Add new entries here as their backend
     *  arm in `brush_graph.rs::brush_node_preview` lands. */
    const PREVIEWABLE_NODE_TYPES = new Set(['noise']);
    let isPreviewable = $derived(PREVIEWABLE_NODE_TYPES.has(node.type_id));

    // Node type info for display name.
    let typeInfo = $derived(brushGraph.getNodeType(node.type_id));
    let displayName = $derived(typeInfo?.display_name ?? node.type_id);

    /** Apply the port's `visible_when` rule against the referenced sibling
     *  input's current value. Engine-side the port still works regardless —
     *  this is purely UI. */
    function isPortVisible(port: PortDef): boolean {
        if (!port.visible_when) return true;
        const [inputName, allowed] = port.visible_when;
        const src = node.ports.find(p => p.name === inputName && p.dir === 'Input');
        if (!src) return true;
        return allowed.includes(Number(src.value));
    }
    let inputPorts = $derived(
        node.ports.filter(p => p.dir === 'Input' && isPortVisible(p))
    );

    // --- Drag to move (from any point on the node) ---
    // Updates `brushGraph.nodePositions` directly — positions are
    // UI-only state and never round-trip to Rust.
    let dragging = false;
    let dragStartX = 0;
    let dragStartY = 0;
    let nodeStartX = 0;
    let nodeStartY = 0;
    let nodeEl: HTMLDivElement;

    /** Returns true if the event target is an interactive child that should
     *  handle its own pointer events (port dots, sliders, buttons). */
    function isInteractiveTarget(e: PointerEvent): boolean {
        const t = e.target as HTMLElement;
        return !!t.closest('.port-dot, .port-slider, .curve-editor, input, button, select');
    }

    function onNodeDown(e: PointerEvent) {
        if (isInteractiveTarget(e)) return;
        e.stopPropagation();
        brushGraph.selectedNode = node.id;
        dragging = true;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        nodeStartX = position[0];
        nodeStartY = position[1];
        nodeEl.setPointerCapture(e.pointerId);
        app.beginInteraction();
    }

    function onNodeMove(e: PointerEvent) {
        if (!dragging) return;
        const d = coords.clientDeltaToGraph(e.clientX - dragStartX, e.clientY - dragStartY);
        brushGraph.moveNode(node.id, nodeStartX + d.x, nodeStartY + d.y);
    }

    function onNodeUp(e: PointerEvent) {
        if (!dragging) return;
        dragging = false;
        nodeEl.releasePointerCapture(e.pointerId);
    }

    /** Guaranteed cleanup — fires when capture ends for any reason. */
    function onNodeLostCapture() {
        dragging = false;
        app.endInteraction();
    }

    function onRemove(e: MouseEvent) {
        e.stopPropagation();
        brushGraph.removeNode(node.id);
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="node-widget"
    class:selected={isSelected}
    style="transform: translate({position[0]}px, {position[1]}px);"
    data-node-id={node.id}
    bind:this={nodeEl}
    onpointerdown={onNodeDown}
    onpointermove={onNodeMove}
    onpointerup={onNodeUp}
    onlostpointercapture={onNodeLostCapture}
>
    <div class="node-header">
        <span class="node-title">{displayName}</span>
        <button class="remove-btn" onclick={onRemove} title="Remove node">&times;</button>
    </div>

    <div class="node-body">
        {#if outputPorts.length > 0}
            <div class="ports-outputs">
                {#each outputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="right" />
                {/each}
            </div>
        {/if}
        {#if inputPorts.length > 0}
            <div class="ports-inputs">
                {#each inputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="left" />
                {/each}
            </div>
        {/if}

        {#if isPreviewable}
            <NodePreview nodeId={node.id} width={96} height={96} />
        {/if}
    </div>
</div>

<style>
    .node-widget {
        position: absolute;
        left: 0;
        top: 0;
        min-width: 140px;
        background: var(--bg-active);
        border: 1px solid color-mix(in srgb, var(--text) 15%, transparent);
        border-radius: 6px;
        font-size: 11px;
        cursor: grab;
        user-select: none;
    }
    .node-widget:active {
        cursor: grabbing;
    }
    .node-widget.selected {
        border-color: var(--accent);
    }
    .node-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 4px 6px;
        background: var(--bg);
        border-radius: 5px 5px 0 0;
    }
    .node-title {
        font-weight: 600;
        color: var(--text);
        font-size: 10px;
    }
    .remove-btn {
        background: none;
        border: none;
        color: var(--text);
        cursor: pointer;
        font-size: 14px;
        padding: 0 2px;
        line-height: 1;
        transition: color 0.1s;
    }
    .remove-btn:hover {
        color: var(--danger);
    }
    .node-body {
        padding: 4px 0;
    }
    .ports-outputs,
    .ports-inputs {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }
</style>
