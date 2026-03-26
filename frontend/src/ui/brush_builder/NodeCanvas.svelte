<script lang="ts">
    import { onMount } from 'svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
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

    /** Whether we've suppressed the main canvas animation loop.
     *  Tied to pointer capture: set on setPointerCapture, cleared on
     *  lostpointercapture.  The browser guarantees lostpointercapture
     *  fires when capture ends for ANY reason (release, pointer up,
     *  window blur, tab switch, element removal) — so this can never
     *  leak.  Same guarantee as Python's `with:`. */
    let interactionActive = false;

    /** Capture the pointer and suppress the main canvas animation loop. */
    function capturePointer(e: PointerEvent) {
        canvasEl.setPointerCapture(e.pointerId);
        if (!interactionActive) {
            interactionActive = true;
            app.beginInteraction();
        }
    }

    // --- Image upload helpers ---

    /** Open a file picker and upload the chosen image to an Image node. */
    function browseImageForNode(nodeId: number) {
        const input = document.createElement('input');
        input.type = 'file';
        input.accept = 'image/*';
        input.onchange = async () => {
            const file = input.files?.[0];
            if (file) {
                await brushGraph.uploadBlobToNode(nodeId, file);
                renderer?.markDirty();
            }
        };
        input.click();
    }

    /** Upload an image Blob (from drop or paste) to the focused Image node. */
    async function uploadBlobToImageNode(nodeId: number, blob: Blob) {
        await brushGraph.uploadBlobToNode(nodeId, blob);
        renderer?.markDirty();
    }

    /** Find the Image node under a drop point, or the selected Image node. */
    function imageNodeAt(gx: number, gy: number): number | null {
        const hit = renderer?.hitTest(gx, gy);
        if (hit?.type === 'image-upload' && hit.nodeId != null) return hit.nodeId;
        // Fallback: selected node if it's an Image node.
        if (brushGraph.selectedNode != null) {
            const node = brushGraph.graph?.nodes[String(brushGraph.selectedNode)];
            if (node?.type_id === 'image') return brushGraph.selectedNode;
        }
        return null;
    }

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
        renderer.markDirty(true);
    }

    function onPointerDown(e: PointerEvent) {
        if (!renderer) return;

        // Middle-click → pan
        if (e.button === 1) {
            e.preventDefault();
            isPanning = true;
            panStartX = e.clientX; panStartY = e.clientY;
            panOriginX = renderer.panX; panOriginY = renderer.panY;
            capturePointer(e);
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
                capturePointer(e);
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
                        capturePointer(e);
                        break;
                    }
                }
                brushGraph.draggingFrom = { node: nodeId!, port: portName!, dir: portDir! };
                capturePointer(e);
                break;
            }

            case 'param-checkbox': {
                const n2 = brushGraph.graph?.nodes[String(hit.nodeId!)];
                if (n2) {
                    const v = !n2.params[hit.paramIndex!];
                    brushGraph.setParamLocal(hit.nodeId!, hit.paramIndex!, v);
                    brushGraph.setParam(hit.nodeId!, hit.paramIndex!, 'bool', v);
                }
                renderer.markDirty(true);
                break;
            }

            case 'param-slider':
                isDraggingSlider = true;
                sliderNodeId = hit.nodeId!;
                sliderParamIdx = hit.paramIndex!;
                capturePointer(e);
                { const v = renderer.sliderValueAt(hit.nodeId!, hit.paramIndex!, g.x);
                  if (v !== null) brushGraph.setParamLocal(hit.nodeId!, hit.paramIndex!, v); }
                renderer.markDirty(true);
                break;

            case 'image-upload':
                brushGraph.selectedNode = hit.nodeId!;
                browseImageForNode(hit.nodeId!);
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
            renderer.markDirty(true);
            return;
        }

        const g = renderer.screenToGraph(e.clientX, e.clientY);

        if (isDraggingNode) {
            brushGraph.moveNode(dragNodeId, nodeStartX + (g.x - dragStartGX), nodeStartY + (g.y - dragStartGY));
            renderer.markDirty(true);
            return;
        }

        if (isDraggingSlider) {
            const v = renderer.sliderValueAt(sliderNodeId, sliderParamIdx, g.x);
            if (v !== null) brushGraph.setParamLocal(sliderNodeId, sliderParamIdx, v);
            renderer.markDirty(true);
            return;
        }

        if (brushGraph.draggingFrom) {
            brushGraph.dragMouse = { x: g.x, y: g.y };
            renderer.markDirty(true);
        }
    }

    function onPointerUp(e: PointerEvent) {
        if (!renderer) return;

        // Semantic cleanup for each drag type.  The interaction gating
        // cleanup (endInteraction) happens in onLostCapture below —
        // guaranteed by the browser regardless of how capture ends.

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

    /** Guaranteed cleanup — fires when pointer capture ends for ANY
     *  reason: explicit release, pointer up, window blur, tab switch,
     *  element removal.  This is the single place that resumes the
     *  main canvas animation loop. */
    function onLostCapture() {
        isPanning = false;
        isDraggingNode = false;
        isDraggingSlider = false;
        if (interactionActive) {
            interactionActive = false;
            app.endInteraction();
        }
    }

    function onContextMenu(e: MouseEvent) { e.preventDefault(); }

    // --- Drag & drop image onto Image nodes ---

    function onDragOver(e: DragEvent) {
        if (e.dataTransfer?.types.some(t => t === 'Files' || t.startsWith('image/'))) {
            e.preventDefault();
            e.dataTransfer!.dropEffect = 'copy';
        }
    }

    async function onDrop(e: DragEvent) {
        e.preventDefault();
        if (!renderer || !e.dataTransfer) return;
        const g = renderer.screenToGraph(e.clientX, e.clientY);
        const nodeId = imageNodeAt(g.x, g.y);
        if (nodeId == null) return;

        // Check for image files.
        for (const file of Array.from(e.dataTransfer.files)) {
            if (file.type.startsWith('image/')) {
                await uploadBlobToImageNode(nodeId, file);
                return;
            }
        }
    }


</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<canvas
    class="node-canvas"
    bind:this={canvasEl}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onlostpointercapture={onLostCapture}
    oncontextmenu={onContextMenu}
    ondragover={onDragOver}
    ondrop={onDrop}
></canvas>

<style>
    .node-canvas {
        flex: 1;
        cursor: default;
        display: block;
    }
</style>
