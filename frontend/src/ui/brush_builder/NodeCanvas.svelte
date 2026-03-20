<script lang="ts" module>
    /** Context type for port offset registration. */
    export interface PortRegistration {
        register(nodeId: number, portName: string, dir: string, offset: { x: number; y: number }): void;
        unregister(nodeId: number, portName: string, dir: string): void;
    }
</script>

<script lang="ts">
    import { setContext } from 'svelte';
    import { brushGraph, WIRE_COLORS, type Connection } from '../../state/brush_graph.svelte';
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

    let canvasEl: HTMLDivElement;

    // --- Port offset cache ---
    // Offsets are measured once per port on mount, relative to the node-widget origin.
    // They never change (port layout within a node is fixed by CSS).
    const portOffsets = new Map<string, { x: number; y: number }>();
    let portOffsetsVersion = $state(0);

    function portKey(nodeId: number, portName: string, dir: string): string {
        return `${nodeId}:${portName}:${dir}`;
    }

    setContext<PortRegistration>('port-registration', {
        register(nodeId, portName, dir, offset) {
            portOffsets.set(portKey(nodeId, portName, dir), offset);
            portOffsetsVersion++;
        },
        unregister(nodeId, portName, dir) {
            portOffsets.delete(portKey(nodeId, portName, dir));
            portOffsetsVersion++;
        },
    });

    /** Resolve a port's position in graph coordinates from cached offsets. */
    function resolvePortPosition(nodeId: number, portName: string, dir: string): { x: number; y: number } | null {
        const offset = portOffsets.get(portKey(nodeId, portName, dir));
        if (!offset) return null;
        const node = brushGraph.graph?.nodes[String(nodeId)];
        if (!node) return null;
        return {
            x: node.position[0] + offset.x,
            y: node.position[1] + offset.y,
        };
    }

    function bezierPathFromPoints(from: { x: number; y: number }, to: { x: number; y: number }): string {
        const dx = Math.abs(to.x - from.x) * 0.5;
        const cpx1 = from.x + Math.max(dx, 30);
        const cpx2 = to.x - Math.max(dx, 30);
        return `M ${from.x} ${from.y} C ${cpx1} ${from.y}, ${cpx2} ${to.y}, ${to.x} ${to.y}`;
    }

    // Pre-compute all wire paths. Depends on connections, node positions, and port offsets.
    // Does NOT depend on panX/panY/zoom — those only affect the SVG <g transform>.
    let wirePaths = $derived.by(() => {
        const conns = brushGraph.connectionList;
        const _v = portOffsetsVersion; // track offset changes
        const _g = brushGraph.graph;   // track node position changes

        const result: { path: string; color: string }[] = [];
        for (const conn of conns) {
            const from = resolvePortPosition(conn.from.node, conn.from.port, 'Output');
            const to = resolvePortPosition(conn.to.node, conn.to.port, 'Input');
            if (!from || !to) continue;
            const wt = brushGraph.getPortWireType(conn.from.node, conn.from.port);
            result.push({
                path: bezierPathFromPoints(from, to),
                color: wt ? (WIRE_COLORS[wt] ?? '#888') : '#888',
            });
        }
        return result;
    });

    // Drag wire: compute from dragging port position + mouse.
    let dragWire = $derived.by(() => {
        const drag = brushGraph.draggingFrom;
        const mouse = brushGraph.dragMouse;
        if (!drag || !mouse) return null;
        const _v = portOffsetsVersion;
        const _g = brushGraph.graph;
        const portPos = resolvePortPosition(drag.node, drag.port, drag.dir);
        if (!portPos) return null;
        const from = drag.dir === 'Output' ? portPos : mouse;
        const to = drag.dir === 'Output' ? mouse : portPos;
        const wt = brushGraph.getPortWireType(drag.node, drag.port);
        return {
            path: bezierPathFromPoints(from, to),
            color: wt ? (WIRE_COLORS[wt] ?? '#888') : '#888',
        };
    });

    function onWheel(e: WheelEvent) {
        e.preventDefault();
        if (e.ctrlKey || e.metaKey) {
            const factor = e.deltaY > 0 ? 0.9 : 1.1;
            const newZoom = Math.max(0.2, Math.min(3, zoom * factor));
            const rect = canvasEl.getBoundingClientRect();
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
        if (e.button === 1) {
            e.preventDefault();
            isPanning = true;
            panStartX = e.clientX;
            panStartY = e.clientY;
            panOriginX = panX;
            panOriginY = panY;
            canvasEl.setPointerCapture(e.pointerId);
        } else if (e.button === 0) {
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
        if (brushGraph.draggingFrom) {
            brushGraph.draggingFrom = null;
            brushGraph.dragMouse = null;
        }
    }

    function onContextMenu(e: MouseEvent) {
        e.preventDefault();
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
