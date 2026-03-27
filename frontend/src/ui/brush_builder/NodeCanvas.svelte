<script lang="ts">
    import { setContext, tick } from 'svelte';
    import { brushGraph, WIRE_COLORS } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
    import NodeWidget from './NodeWidget.svelte';
    import WireRenderer from './WireRenderer.svelte';

    // --- Auto-layout when nodes have no positions ---
    // $effect runs after DOM is updated, so node elements can be measured.
    $effect(() => {
        if (!brushGraph.needsLayout) return;
        const sizes: Record<string, [number, number]> = {};
        for (const el of document.querySelectorAll<HTMLElement>('[data-node-id]')) {
            const id = el.dataset.nodeId;
            if (id) sizes[id] = [el.offsetWidth, el.offsetHeight];
        }
        if (Object.keys(sizes).length > 0) {
            brushGraph.autoLayout(sizes);
        }
    });

    // --- Port offset registration ---
    // PortWidget measures its dot's offset relative to its node on mount
    // and registers it here.  Wire paths use node.position + offset.

    export interface PortRegistration {
        register(nodeId: number, portName: string, dir: string, offset: { x: number; y: number }): void;
        unregister(nodeId: number, portName: string, dir: string): void;
    }

    const portOffsets = new Map<string, { x: number; y: number }>();
    /** Bumped on every register/unregister so $derived picks up changes. */
    let portVersion = $state(0);

    setContext<PortRegistration>('port-registration', {
        register(nodeId, portName, dir, offset) {
            portOffsets.set(`${nodeId}:${portName}:${dir}`, offset);
            portVersion++;
        },
        unregister(nodeId, portName, dir) {
            portOffsets.delete(`${nodeId}:${portName}:${dir}`);
            portVersion++;
        },
    });

    // --- Pan / zoom ---

    let containerEl: HTMLDivElement;
    let panX = $state(0);
    let panY = $state(0);
    let zoom = $state(1);

    // --- Interaction state ---

    let isPanning = false;
    let panStartX = 0, panStartY = 0, panOriginX = 0, panOriginY = 0;
    let interactionActive = false;

    function capturePointer(e: PointerEvent) {
        containerEl.setPointerCapture(e.pointerId);
        if (!interactionActive) {
            interactionActive = true;
            app.beginInteraction();
        }
    }

    // --- Wire path computation ---

    function portWorldPos(nodeId: number, portName: string, dir: string) {
        const node = brushGraph.graph?.nodes[String(nodeId)];
        if (!node) return null;
        const key = `${nodeId}:${portName}:${dir}`;
        let offset = portOffsets.get(key);
        if (!offset) {
            // onMount hasn't fired yet — measure directly from the DOM.
            const dotEl = document.querySelector(
                `[data-port-node="${nodeId}"][data-port-name="${portName}"][data-port-dir="${dir}"]`
            ) as HTMLElement | null;
            const nodeEl = dotEl?.closest('[data-node-id]') as HTMLElement | null;
            if (dotEl && nodeEl) {
                const dotRect = dotEl.getBoundingClientRect();
                const nodeRect = nodeEl.getBoundingClientRect();
                offset = {
                    x: (dotRect.left + dotRect.width / 2) - nodeRect.left,
                    y: (dotRect.top + dotRect.height / 2) - nodeRect.top,
                };
                portOffsets.set(key, offset);
            }
        }
        if (!offset) return null;
        return { x: node.position[0] + offset.x, y: node.position[1] + offset.y };
    }

    function bezierPath(from: { x: number; y: number }, to: { x: number; y: number }): string {
        const dx = Math.abs(to.x - from.x) * 0.5;
        const cp = Math.max(dx, 30);
        return `M${from.x},${from.y} C${from.x + cp},${from.y} ${to.x - cp},${to.y} ${to.x},${to.y}`;
    }

    let wirePaths = $derived.by(() => {
        portVersion;  // reactive dependency on port registration changes
        const paths: { path: string; color: string }[] = [];
        for (const conn of brushGraph.connectionList) {
            const from = portWorldPos(conn.from.node, conn.from.port, 'Output');
            const to   = portWorldPos(conn.to.node,   conn.to.port,   'Input');
            if (!from || !to) continue;
            const wt = brushGraph.getPortWireType(conn.from.node, conn.from.port);
            paths.push({ path: bezierPath(from, to), color: wt ? (WIRE_COLORS[wt] ?? '#888') : '#888' });
        }
        return paths;
    });

    let dragWire = $derived.by(() => {
        portVersion;
        const drag = brushGraph.draggingFrom;
        const mouse = brushGraph.dragMouse;
        if (!drag || !mouse) return null;
        const pp = portWorldPos(drag.node, drag.port, drag.dir);
        if (!pp) return null;
        const from = drag.dir === 'Output' ? pp : mouse;
        const to   = drag.dir === 'Output' ? mouse : pp;
        const wt = brushGraph.getPortWireType(drag.node, drag.port);
        return { path: bezierPath(from, to), color: wt ? (WIRE_COLORS[wt] ?? '#888') : '#888' };
    });

    // --- Coordinate conversion ---

    function screenToGraph(sx: number, sy: number) {
        const r = containerEl.getBoundingClientRect();
        return { x: (sx - r.left - panX) / zoom, y: (sy - r.top - panY) / zoom };
    }

    // --- Wheel interaction gating ---
    // Wheel events fire in bursts.  Suppress the main canvas render on
    // the first event; resume after a short idle (no capture to attach
    // lostpointercapture to, so we use a debounce timer instead).

    let wheelTimer = 0;

    function wheelBegin() {
        if (!wheelTimer) app.beginInteraction();
        else clearTimeout(wheelTimer);
        wheelTimer = setTimeout(() => { wheelTimer = 0; app.endInteraction(); }, 150) as unknown as number;
    }

    // --- Events ---

    function onWheel(e: WheelEvent) {
        e.preventDefault();
        wheelBegin();
        if (e.ctrlKey || e.metaKey) {
            const factor = e.deltaY > 0 ? 0.9 : 1.1;
            const newZoom = Math.max(0.2, Math.min(3, zoom * factor));
            const rect = containerEl.getBoundingClientRect();
            const mx = e.clientX - rect.left;
            const my = e.clientY - rect.top;
            panX = mx - (mx - panX) * (newZoom / zoom);
            panY = my - (my - panY) * (newZoom / zoom);
            zoom = newZoom;
        } else {
            panX -= e.deltaX;
            panY -= e.deltaY;
        }
    }

    function onPointerDown(e: PointerEvent) {
        // Middle-click → pan
        if (e.button === 1) {
            e.preventDefault();
            isPanning = true;
            panStartX = e.clientX; panStartY = e.clientY;
            panOriginX = panX; panOriginY = panY;
            capturePointer(e);
            return;
        }

        // Wire drag bubbled up from PortWidget
        if (brushGraph.draggingFrom) {
            capturePointer(e);
            return;
        }

        // Left-click on background → deselect
        if (e.button === 0 && e.target === containerEl) {
            brushGraph.selectedNode = null;
        }
    }

    function onPointerMove(e: PointerEvent) {
        if (isPanning) {
            panX = panOriginX + (e.clientX - panStartX);
            panY = panOriginY + (e.clientY - panStartY);
            return;
        }
        if (brushGraph.draggingFrom) {
            brushGraph.dragMouse = screenToGraph(e.clientX, e.clientY);
        }
    }

    function onPointerUp(e: PointerEvent) {
        if (isPanning) {
            isPanning = false;
            containerEl.releasePointerCapture(e.pointerId);
            return;
        }
        if (brushGraph.draggingFrom) {
            // Pointer capture prevents the target port from seeing pointerup.
            // Release capture and hit-test to find the port dot under the pointer.
            if (containerEl.hasPointerCapture(e.pointerId)) {
                containerEl.releasePointerCapture(e.pointerId);
            }
            const target = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null;
            const portEl = target?.closest('[data-port-node]') as HTMLElement | null;
            if (portEl) {
                const drag = brushGraph.draggingFrom;
                const targetNode = Number(portEl.dataset.portNode);
                const targetPort = portEl.dataset.portName!;
                const targetDir = portEl.dataset.portDir as 'Input' | 'Output';

                // Don't connect to self.
                if (!(drag.node === targetNode && drag.port === targetPort)) {
                    if (drag.dir === 'Output' && targetDir === 'Input') {
                        brushGraph.connect(drag.node, drag.port, targetNode, targetPort);
                    } else if (drag.dir === 'Input' && targetDir === 'Output') {
                        brushGraph.connect(targetNode, targetPort, drag.node, drag.port);
                    }
                }
            }
            brushGraph.draggingFrom = null;
            brushGraph.dragMouse = null;
        }
    }

    /** Guaranteed cleanup for interaction gating. */
    function onLostCapture() {
        isPanning = false;
        if (interactionActive) {
            interactionActive = false;
            app.endInteraction();
        }
    }

    // --- Image upload: drag & drop onto the container ---

    function onDragOver(e: DragEvent) {
        if (e.dataTransfer?.types.some(t => t === 'Files' || t.startsWith('image/'))) {
            e.preventDefault();
            e.dataTransfer!.dropEffect = 'copy';
        }
    }

    async function onDrop(e: DragEvent) {
        e.preventDefault();
        if (!e.dataTransfer) return;
        // Find the Image node under the drop point, or the selected one.
        const g = screenToGraph(e.clientX, e.clientY);
        let nodeId: number | null = null;
        if (brushGraph.selectedNode != null) {
            const node = brushGraph.graph?.nodes[String(brushGraph.selectedNode)];
            if (node?.type_id === 'image') nodeId = brushGraph.selectedNode;
        }
        if (nodeId == null) return;
        for (const file of Array.from(e.dataTransfer.files)) {
            if (file.type.startsWith('image/')) {
                await brushGraph.uploadBlobToNode(nodeId, file);
                return;
            }
        }
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="graph-container"
    bind:this={containerEl}
    style="background-position: {panX}px {panY}px; background-size: {20 * zoom}px {20 * zoom}px;"
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onlostpointercapture={onLostCapture}
    oncontextmenu={(e) => e.preventDefault()}
    ondragover={onDragOver}
    ondrop={onDrop}
>
    <WireRenderer {wirePaths} {dragWire} {panX} {panY} {zoom} />

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
    .graph-container {
        position: relative;
        flex: 1;
        overflow: hidden;
        background-color: var(--thumb-bg);
        background-image: radial-gradient(circle, color-mix(in srgb, var(--text) 40%, transparent) 1px, transparent 1px);
        background-size: 20px 20px;
        cursor: default;
    }
    .node-layer {
        position: absolute;
        inset: 0;
    }
</style>
