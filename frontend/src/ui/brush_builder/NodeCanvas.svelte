<script lang="ts">
    import { tick } from 'svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import NodeWidget from './NodeWidget.svelte';
    import WireRenderer from './WireRenderer.svelte';

    // --- Pan / zoom state ---
    let panX = $state(0);
    let panY = $state(0);
    let zoom = $state(1);

    let isPanning = $state(false);
    let panStartX = 0;
    let panStartY = 0;
    let panOriginX = 0;
    let panOriginY = 0;

    // Bumped after DOM updates so WireRenderer re-queries port positions.
    let wireTick = $state(0);

    let canvasEl: HTMLDivElement;

    // Re-query wire positions after any change to nodes or connections.
    $effect(() => {
        brushGraph.nodeList;
        brushGraph.connectionList;
        tick().then(() => { wireTick++; });
    });

    function onWheel(e: WheelEvent) {
        e.preventDefault();
        if (e.ctrlKey || e.metaKey) {
            // Pinch-to-zoom (trackpad) or ctrl+scroll (mouse).
            const factor = e.deltaY > 0 ? 0.9 : 1.1;
            const newZoom = Math.max(0.2, Math.min(3, zoom * factor));
            const rect = canvasEl.getBoundingClientRect();
            const mx = e.clientX - rect.left;
            const my = e.clientY - rect.top;
            panX = mx - (mx - panX) * (newZoom / zoom);
            panY = my - (my - panY) * (newZoom / zoom);
            zoom = newZoom;
        } else {
            // Two-finger scroll → pan.
            panX -= e.deltaX;
            panY -= e.deltaY;
        }
    }

    function onPointerDown(e: PointerEvent) {
        // Middle-click to pan.
        if (e.button === 1) {
            e.preventDefault();
            isPanning = true;
            panStartX = e.clientX;
            panStartY = e.clientY;
            panOriginX = panX;
            panOriginY = panY;
            canvasEl.setPointerCapture(e.pointerId);
        } else if (e.button === 0) {
            // Deselect when clicking on empty space.
            if (e.target === canvasEl || (e.target as HTMLElement).classList.contains('node-layer')) {
                brushGraph.selectedNode = null;
            }
        }
    }

    function onPointerMove(e: PointerEvent) {
        if (isPanning) {
            panX = panOriginX + (e.clientX - panStartX);
            panY = panOriginY + (e.clientY - panStartY);
        }
        if (brushGraph.draggingFrom && canvasEl) {
            const rect = canvasEl.getBoundingClientRect();
            brushGraph.dragMouse = {
                x: (e.clientX - rect.left - panX) / zoom,
                y: (e.clientY - rect.top - panY) / zoom,
            };
        }
    }

    function onPointerUp(e: PointerEvent) {
        if (isPanning) {
            isPanning = false;
            canvasEl.releasePointerCapture(e.pointerId);
        }
        // Clear drag state on any mouseup over the canvas.
        if (brushGraph.draggingFrom) {
            brushGraph.draggingFrom = null;
            brushGraph.dragMouse = null;
        }
    }

    function onContextMenu(e: MouseEvent) {
        e.preventDefault();
    }

    /**
     * Get the position of a port dot in canvas-local coordinates.
     * Used by WireRenderer to draw bezier curves.
     */
    function getPortPosition(nodeId: number, portName: string, dir: 'Input' | 'Output'): { x: number; y: number } | null {
        if (!canvasEl) return null;
        const selector = `[data-port-node="${nodeId}"][data-port-name="${portName}"][data-port-dir="${dir}"]`;
        const dot = canvasEl.querySelector(selector) as HTMLElement | null;
        if (!dot) return null;

        const dotRect = dot.getBoundingClientRect();
        const canvasRect = canvasEl.getBoundingClientRect();

        // Convert from screen to canvas-local, then undo pan/zoom to get graph coordinates.
        const screenX = dotRect.left + dotRect.width / 2 - canvasRect.left;
        const screenY = dotRect.top + dotRect.height / 2 - canvasRect.top;
        return {
            x: (screenX - panX) / zoom,
            y: (screenY - panY) / zoom,
        };
    }
</script>

<div
    class="node-canvas"
    bind:this={canvasEl}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    oncontextmenu={onContextMenu}
    role="application"
    tabindex="-1"
>
    {#key wireTick}
    <WireRenderer
        connections={brushGraph.connectionList}
        {getPortPosition}
        {panX}
        {panY}
        {zoom}
        draggingFrom={brushGraph.draggingFrom}
        dragMouse={brushGraph.dragMouse}
    />
    {/key}

    <div
        class="node-layer"
        style="transform: translate({panX}px, {panY}px) scale({zoom}); transform-origin: 0 0;"
    >
        {#each brushGraph.nodeList as node (node.id)}
            <NodeWidget {node} {zoom} />
        {/each}
    </div>
</div>

<style>
    .node-canvas {
        position: relative;
        flex: 1;
        overflow: hidden;
        background: #1a1a1a;
        background-image:
            radial-gradient(circle, #333 1px, transparent 1px);
        background-size: 20px 20px;
        cursor: default;
    }
    .node-layer {
        position: absolute;
        inset: 0;
        pointer-events: none;
    }
    .node-layer > :global(*) {
        pointer-events: auto;
    }
</style>
