<script lang="ts">
    import { brushGraph, WIRE_COLORS, type PortDef } from '../../state/brush_graph.svelte';

    interface Props {
        nodeId: number;
        port: PortDef;
        side: 'left' | 'right';
    }

    let { nodeId, port, side }: Props = $props();

    let color = $derived(WIRE_COLORS[port.wire_type] ?? '#888');
    let connected = $derived(brushGraph.isPortConnected(nodeId, port.name, port.dir));

    function onPointerDown(e: PointerEvent) {
        e.stopPropagation();
        e.preventDefault();

        // If dragging from a connected input, detach the wire and drag from the output end.
        if (port.dir === 'Input' && connected) {
            const conn = brushGraph.connectionList.find(
                c => c.to.node === nodeId && c.to.port === port.name
            );
            if (conn) {
                brushGraph.disconnect(conn.from.node, conn.from.port, conn.to.node, conn.to.port);
                brushGraph.draggingFrom = {
                    node: conn.from.node,
                    port: conn.from.port,
                    dir: 'Output',
                };
                return;
            }
        }

        brushGraph.draggingFrom = {
            node: nodeId,
            port: port.name,
            dir: port.dir,
        };
    }

    function onPointerUp(e: PointerEvent) {
        e.stopPropagation();
        e.preventDefault();
        const drag = brushGraph.draggingFrom;
        if (!drag) return;

        // Can't connect to self.
        if (drag.node === nodeId && drag.port === port.name) {
            brushGraph.draggingFrom = null;
            brushGraph.dragMouse = null;
            return;
        }

        // Determine from/to based on direction.
        if (drag.dir === 'Output' && port.dir === 'Input') {
            brushGraph.connect(drag.node, drag.port, nodeId, port.name);
        } else if (drag.dir === 'Input' && port.dir === 'Output') {
            brushGraph.connect(nodeId, port.name, drag.node, drag.port);
        }
        brushGraph.draggingFrom = null;
        brushGraph.dragMouse = null;
    }
</script>

<div
    class="port-row"
    class:port-right={side === 'right'}
>
    <div
        class="port-dot"
        class:connected
        style="background: {connected ? color : 'transparent'}; border-color: {color};"
        role="button"
        tabindex="-1"
        onpointerdown={onPointerDown}
        onpointerup={onPointerUp}
        data-port-node={nodeId}
        data-port-name={port.name}
        data-port-dir={port.dir}
    ></div>
    <span class="port-label">{port.name}</span>
</div>

<style>
    .port-row {
        display: flex;
        align-items: center;
        gap: 4px;
        height: 18px;
    }
    .port-right {
        flex-direction: row-reverse;
    }
    .port-dot {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 2px solid;
        cursor: crosshair;
        flex-shrink: 0;
    }
    .port-dot.connected {
        /* filled by inline style */
    }
    .port-dot:hover {
        transform: scale(1.3);
    }
    .port-label {
        font-size: 9px;
        color: #bbb;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
</style>
