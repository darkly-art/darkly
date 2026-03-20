<script lang="ts">
    import { onMount } from 'svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { CanvasRenderer } from './canvas_renderer';

    let canvasEl: HTMLCanvasElement;
    let renderer: CanvasRenderer;

    // --- Interaction state (plain vars, no Svelte reactivity) ---
    let isPanning = false;
    let panStartX = 0, panStartY = 0, panOriginX = 0, panOriginY = 0;

    let isDraggingNode = false;
    let dragNodeId = 0;
    let dragStartGX = 0, dragStartGY = 0, nodeStartX = 0, nodeStartY = 0;

    let isDraggingSlider = false;
    let sliderNodeId = 0, sliderParamIdx = 0;

    onMount(() => {
        renderer = new CanvasRenderer(canvasEl);
        renderer.start();
        const ro = new ResizeObserver(() => renderer.resize());
        ro.observe(canvasEl);
        return () => { renderer.stop(); ro.disconnect(); };
    });

    // Mark canvas dirty when graph structure changes (add/remove node,
    // connect/disconnect, selection change).  Deep mutations during drag
    // (position, params) call markDirty() directly from event handlers
    // because Svelte 5's $effect only tracks the properties actually read
    // here — not nested mutations within the graph proxy.
    $effect(() => {
        if (!renderer) return;
        brushGraph.graph;
        brushGraph.selectedNode;
        brushGraph.draggingFrom;
        brushGraph.dragMouse;
        renderer.markDirty();
    });

    // --- Events ---

    function onWheel(e: WheelEvent) {
        e.preventDefault();
        if (!renderer) return;
        if (e.ctrlKey || e.metaKey) {
            const factor = e.deltaY > 0 ? 0.9 : 1.1;
            const newZoom = Math.max(0.2, Math.min(3, renderer.zoom * factor));
            const rect = canvasEl.getBoundingClientRect();
            const mx = e.clientX - rect.left;
            const my = e.clientY - rect.top;
            renderer.panX = mx - (mx - renderer.panX) * (newZoom / renderer.zoom);
            renderer.panY = my - (my - renderer.panY) * (newZoom / renderer.zoom);
            renderer.zoom = newZoom;
        } else {
            renderer.panX -= e.deltaX;
            renderer.panY -= e.deltaY;
        }
        renderer.markDirty();
    }

    function onPointerDown(e: PointerEvent) {
        if (!renderer) return;

        // Middle-click → pan
        if (e.button === 1) {
            e.preventDefault();
            isPanning = true;
            panStartX = e.clientX; panStartY = e.clientY;
            panOriginX = renderer.panX; panOriginY = renderer.panY;
            canvasEl.setPointerCapture(e.pointerId);
            return;
        }
        if (e.button !== 0) return;

        const g = renderer.screenToGraph(e.clientX, e.clientY);
        const hit = renderer.hitTest(g.x, g.y);

        switch (hit.type) {
            case 'remove-btn':
                brushGraph.removeNode(hit.nodeId!);
                break;

            case 'node-header': {
                brushGraph.selectedNode = hit.nodeId!;
                isDraggingNode = true;
                dragNodeId = hit.nodeId!;
                dragStartGX = g.x; dragStartGY = g.y;
                const n = brushGraph.graph?.nodes[String(hit.nodeId!)];
                if (n) { nodeStartX = n.position[0]; nodeStartY = n.position[1]; }
                canvasEl.setPointerCapture(e.pointerId);
                break;
            }

            case 'port': {
                const { nodeId, portName, portDir } = hit;
                // Detach if dragging from a connected input
                if (portDir === 'Input' && brushGraph.isPortConnected(nodeId!, portName!, 'Input')) {
                    const conn = brushGraph.connectionList.find(
                        c => c.to.node === nodeId && c.to.port === portName,
                    );
                    if (conn) {
                        brushGraph.disconnect(conn.from.node, conn.from.port, conn.to.node, conn.to.port);
                        brushGraph.draggingFrom = { node: conn.from.node, port: conn.from.port, dir: 'Output' };
                        canvasEl.setPointerCapture(e.pointerId);
                        break;
                    }
                }
                brushGraph.draggingFrom = { node: nodeId!, port: portName!, dir: portDir! };
                canvasEl.setPointerCapture(e.pointerId);
                break;
            }

            case 'param-checkbox': {
                const n2 = brushGraph.graph?.nodes[String(hit.nodeId!)];
                if (n2) {
                    const v = !n2.params[hit.paramIndex!];
                    brushGraph.setParamLocal(hit.nodeId!, hit.paramIndex!, v);
                    brushGraph.setParam(hit.nodeId!, hit.paramIndex!, 'bool', v);
                }
                renderer.markDirty();
                break;
            }

            case 'param-slider':
                isDraggingSlider = true;
                sliderNodeId = hit.nodeId!;
                sliderParamIdx = hit.paramIndex!;
                canvasEl.setPointerCapture(e.pointerId);
                { const v = renderer.sliderValueAt(hit.nodeId!, hit.paramIndex!, g.x);
                  if (v !== null) brushGraph.setParamLocal(hit.nodeId!, hit.paramIndex!, v); }
                renderer.markDirty();
                break;

            case 'node-body':
                brushGraph.selectedNode = hit.nodeId!;
                break;

            case 'none':
                brushGraph.selectedNode = null;
                break;
        }
    }

    function onPointerMove(e: PointerEvent) {
        if (!renderer) return;

        if (isPanning) {
            renderer.panX = panOriginX + (e.clientX - panStartX);
            renderer.panY = panOriginY + (e.clientY - panStartY);
            renderer.markDirty();
            return;
        }

        const g = renderer.screenToGraph(e.clientX, e.clientY);

        if (isDraggingNode) {
            brushGraph.moveNode(dragNodeId, nodeStartX + (g.x - dragStartGX), nodeStartY + (g.y - dragStartGY));
            renderer.markDirty();
            return;
        }

        if (isDraggingSlider) {
            const v = renderer.sliderValueAt(sliderNodeId, sliderParamIdx, g.x);
            if (v !== null) brushGraph.setParamLocal(sliderNodeId, sliderParamIdx, v);
            renderer.markDirty();
            return;
        }

        if (brushGraph.draggingFrom) {
            brushGraph.dragMouse = { x: g.x, y: g.y };
            renderer.markDirty();
        }
    }

    function onPointerUp(e: PointerEvent) {
        if (!renderer) return;

        if (isPanning) {
            isPanning = false;
            canvasEl.releasePointerCapture(e.pointerId);
            return;
        }

        if (isDraggingNode) {
            isDraggingNode = false;
            canvasEl.releasePointerCapture(e.pointerId);
            brushGraph.syncNodePosition(dragNodeId);
            return;
        }

        if (isDraggingSlider) {
            isDraggingSlider = false;
            canvasEl.releasePointerCapture(e.pointerId);
            const node = brushGraph.graph?.nodes[String(sliderNodeId)];
            if (node) {
                const ti = brushGraph.getNodeType(node.type_id);
                const pd = (ti?.params as any)?.[sliderParamIdx];
                if (pd) brushGraph.setParam(sliderNodeId, sliderParamIdx, pd.kind, node.params[sliderParamIdx]);
            }
            return;
        }

        if (brushGraph.draggingFrom) {
            const g = renderer.screenToGraph(e.clientX, e.clientY);
            const hit = renderer.hitTest(g.x, g.y);
            if (hit.type === 'port') {
                const drag = brushGraph.draggingFrom;
                if (!(drag.node === hit.nodeId && drag.port === hit.portName)) {
                    if (drag.dir === 'Output' && hit.portDir === 'Input')
                        brushGraph.connect(drag.node, drag.port, hit.nodeId!, hit.portName!);
                    else if (drag.dir === 'Input' && hit.portDir === 'Output')
                        brushGraph.connect(hit.nodeId!, hit.portName!, drag.node, drag.port);
                }
            }
            brushGraph.draggingFrom = null;
            brushGraph.dragMouse = null;
            canvasEl.releasePointerCapture(e.pointerId);
        }
    }

    function onContextMenu(e: MouseEvent) { e.preventDefault(); }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<canvas
    class="node-canvas"
    bind:this={canvasEl}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    oncontextmenu={onContextMenu}
></canvas>

<style>
    .node-canvas {
        flex: 1;
        cursor: default;
        display: block;
    }
</style>
