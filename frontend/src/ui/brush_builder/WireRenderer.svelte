<script lang="ts">
    import { brushGraph, WIRE_COLORS, type Connection } from '../../state/brush_graph.svelte';

    interface Props {
        connections: Connection[];
        getPortPosition: (nodeId: number, portName: string, dir: 'Input' | 'Output') => { x: number; y: number } | null;
        panX: number;
        panY: number;
        zoom: number;
        draggingFrom: { node: number; port: string; dir: 'Input' | 'Output' } | null;
        dragMouse: { x: number; y: number } | null;
    }

    let { connections, getPortPosition, panX, panY, zoom, draggingFrom, dragMouse }: Props = $props();

    function wireColor(conn: Connection): string {
        const wt = brushGraph.getPortWireType(conn.from.node, conn.from.port);
        return wt ? (WIRE_COLORS[wt] ?? '#888') : '#888';
    }

    function bezierPathFromPoints(from: { x: number; y: number }, to: { x: number; y: number }): string {
        const dx = Math.abs(to.x - from.x) * 0.5;
        const cpx1 = from.x + Math.max(dx, 30);
        const cpx2 = to.x - Math.max(dx, 30);
        return `M ${from.x} ${from.y} C ${cpx1} ${from.y}, ${cpx2} ${to.y}, ${to.x} ${to.y}`;
    }

    function bezierPath(conn: Connection): string | null {
        const from = getPortPosition(conn.from.node, conn.from.port, 'Output');
        const to = getPortPosition(conn.to.node, conn.to.port, 'Input');
        if (!from || !to) return null;
        return bezierPathFromPoints(from, to);
    }

    function dragWirePath(): string | null {
        if (!draggingFrom || !dragMouse) return null;
        const portPos = getPortPosition(draggingFrom.node, draggingFrom.port, draggingFrom.dir);
        if (!portPos) return null;
        // Output→mouse: port is "from", mouse is "to"
        // Input→mouse: mouse is "from", port is "to"
        if (draggingFrom.dir === 'Output') {
            return bezierPathFromPoints(portPos, dragMouse);
        } else {
            return bezierPathFromPoints(dragMouse, portPos);
        }
    }

    function dragWireColor(): string {
        if (!draggingFrom) return '#888';
        const wt = brushGraph.getPortWireType(draggingFrom.node, draggingFrom.port);
        return wt ? (WIRE_COLORS[wt] ?? '#888') : '#888';
    }
</script>

<svg class="wire-layer">
    <g transform="translate({panX},{panY}) scale({zoom})">
        {#each connections as conn}
            {@const path = bezierPath(conn)}
            {#if path}
                <path
                    d={path}
                    stroke={wireColor(conn)}
                    stroke-width={2 / zoom}
                    fill="none"
                    opacity="0.8"
                />
            {/if}
        {/each}
        {#if dragWirePath()}
            <path
                d={dragWirePath()}
                stroke={dragWireColor()}
                stroke-width={2 / zoom}
                fill="none"
                opacity="0.5"
                stroke-dasharray="{4 / zoom}"
            />
        {/if}
    </g>
</svg>

<style>
    .wire-layer {
        position: absolute;
        inset: 0;
        pointer-events: none;
        overflow: visible;
    }
</style>
