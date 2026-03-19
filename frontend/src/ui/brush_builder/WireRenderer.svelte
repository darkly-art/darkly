<script lang="ts">
    import { brushGraph, WIRE_COLORS, type Connection } from '../../state/brush_graph.svelte';

    interface Props {
        connections: Connection[];
        getPortPosition: (nodeId: number, portName: string, dir: 'Input' | 'Output') => { x: number; y: number } | null;
        panX: number;
        panY: number;
        zoom: number;
    }

    let { connections, getPortPosition, panX, panY, zoom }: Props = $props();

    function wireColor(conn: Connection): string {
        const wt = brushGraph.getPortWireType(conn.from.node, conn.from.port);
        return wt ? (WIRE_COLORS[wt] ?? '#888') : '#888';
    }

    function bezierPath(conn: Connection): string | null {
        const from = getPortPosition(conn.from.node, conn.from.port, 'Output');
        const to = getPortPosition(conn.to.node, conn.to.port, 'Input');
        if (!from || !to) return null;

        const dx = Math.abs(to.x - from.x) * 0.5;
        const cpx1 = from.x + Math.max(dx, 30);
        const cpx2 = to.x - Math.max(dx, 30);
        return `M ${from.x} ${from.y} C ${cpx1} ${from.y}, ${cpx2} ${to.y}, ${to.x} ${to.y}`;
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
