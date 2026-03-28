<script lang="ts">
    import { onMount, onDestroy, getContext } from 'svelte';
    import { brushGraph, WIRE_COLORS, type PortDef } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
    import type { PortRegistration } from './NodeCanvas.svelte';

    interface Props {
        nodeId: number;
        port: PortDef;
        side: 'left' | 'right';
    }

    let { nodeId, port, side }: Props = $props();

    let color = $derived(WIRE_COLORS[port.wire_type] ?? '#888');
    let connected = $derived(brushGraph.isPortConnected(nodeId, port.name, port.dir));

    /** Whether this port should show an inline slider when disconnected. */
    const SLIDER_TYPES = new Set(['Scalar', 'Int', 'Bool']);
    let showSlider = $derived(
        port.dir === 'Input' && !connected && SLIDER_TYPES.has(port.wire_type)
    );

    // --- Port offset registration ---
    const { register, unregister } = getContext<PortRegistration>('port-registration');
    let dotEl = $state<HTMLDivElement>();

    onMount(() => {
        // Measure offset of dot center relative to the ancestor node-widget.
        const nodeEl = dotEl.closest('[data-node-id]') as HTMLElement;
        if (!nodeEl) return;
        const dotRect = dotEl.getBoundingClientRect();
        const nodeRect = nodeEl.getBoundingClientRect();
        register(nodeId, port.name, port.dir, {
            x: (dotRect.left + dotRect.width / 2) - nodeRect.left,
            y: (dotRect.top + dotRect.height / 2) - nodeRect.top,
        });
    });

    onDestroy(() => {
        unregister(nodeId, port.name, port.dir);
    });

    function onPointerDown(e: PointerEvent) {
        // Don't stopPropagation — the container needs to see this event
        // to set up pointer capture for wire drag mouse tracking.
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

    // --- Inline slider for disconnected inputs ---

    let sliderEl = $state<HTMLDivElement>();
    let sliding = false;

    /** Normalized position (0–1) from a pointer event relative to the slider bar. */
    function sliderFraction(e: PointerEvent): number {
        const rect = sliderEl.getBoundingClientRect();
        return Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    }

    function valueFromFraction(frac: number): number {
        const raw = port.min + frac * (port.max - port.min);
        if (port.wire_type === 'Int') return Math.round(raw);
        if (port.wire_type === 'Bool') return frac >= 0.5 ? 1 : 0;
        return raw;
    }

    function onSliderDown(e: PointerEvent) {
        // Stop propagation so the node doesn't start dragging.
        e.stopPropagation();
        e.preventDefault();
        sliding = true;
        sliderEl.setPointerCapture(e.pointerId);
        app.beginInteraction();
        const value = valueFromFraction(sliderFraction(e));
        brushGraph.setPortDefaultLocal(nodeId, port.name, value);
    }

    function onSliderMove(e: PointerEvent) {
        if (!sliding) return;
        const value = valueFromFraction(sliderFraction(e));
        brushGraph.setPortDefaultLocal(nodeId, port.name, value);
    }

    function onSliderUp(e: PointerEvent) {
        if (!sliding) return;
        sliding = false;
        sliderEl.releasePointerCapture(e.pointerId);
        brushGraph.setPortDefault(nodeId, port.name, port.default);
    }

    function onSliderLostCapture() {
        sliding = false;
        app.endInteraction();
    }

    let sliderPercent = $derived(
        port.max > port.min
            ? ((port.default - port.min) / (port.max - port.min)) * 100
            : 0
    );

    let displayValue = $derived(
        port.wire_type === 'Bool'
            ? (port.default >= 0.5 ? 'on' : 'off')
            : port.wire_type === 'Int'
                ? String(Math.round(port.default))
                : port.default.toFixed(2)
    );

    // --- Double-click to type a value ---
    let editing = $state(false);

    function onSliderDblClick(e: MouseEvent) {
        e.stopPropagation();
        e.preventDefault();
        editing = true;
    }

    function onEditKeyDown(e: KeyboardEvent) {
        if (e.key === 'Enter') commitEdit(e.currentTarget as HTMLInputElement);
        if (e.key === 'Escape') editing = false;
    }

    function onEditBlur(e: FocusEvent) {
        commitEdit(e.currentTarget as HTMLInputElement);
    }

    function commitEdit(input: HTMLInputElement) {
        editing = false;
        const parsed = parseFloat(input.value);
        if (isNaN(parsed)) return;
        const clamped = Math.max(port.min, Math.min(port.max, parsed));
        const value = port.wire_type === 'Int' ? Math.round(clamped) : clamped;
        brushGraph.setPortDefaultLocal(nodeId, port.name, value);
        brushGraph.setPortDefault(nodeId, port.name, value);
    }
</script>

<div
    class="port-row"
    class:port-right={side === 'right'}
    class:has-slider={showSlider}
    title={port.description || ''}
>
    <div
        class="port-dot"
        class:connected
        style="background: {connected ? color : 'transparent'}; border-color: {color};"
        role="button"
        tabindex="-1"
        onpointerdown={onPointerDown}
        onpointerup={onPointerUp}
        bind:this={dotEl}
        data-port-node={nodeId}
        data-port-name={port.name}
        data-port-dir={port.dir}
    ></div>
    {#if showSlider}
        {#if editing}
            <!-- svelte-ignore a11y_autofocus -->
            <input
                class="port-slider-edit"
                type="text"
                value={port.wire_type === 'Int' ? Math.round(port.default) : port.default}
                autofocus
                onkeydown={onEditKeyDown}
                onblur={onEditBlur}
                onclick={(e) => e.stopPropagation()}
            />
        {:else}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="port-slider"
                bind:this={sliderEl}
                onpointerdown={onSliderDown}
                onpointermove={onSliderMove}
                onpointerup={onSliderUp}
                onlostpointercapture={onSliderLostCapture}
                ondblclick={onSliderDblClick}
            >
                <div
                    class="port-slider-fill"
                    style="width: {sliderPercent}%; background: {color};"
                ></div>
                <span class="port-slider-label">{port.name}</span>
                <span class="port-slider-value">{displayValue}</span>
            </div>
        {/if}
    {:else}
        <span class="port-label">{port.name}</span>
    {/if}
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
    .port-row.has-slider {
        /* Span full node width so the slider bar stretches across. */
        position: relative;
        margin-right: 2px;
    }
    .port-dot {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 2px solid;
        cursor: crosshair;
        flex-shrink: 0;
        z-index: 1;
    }
    .port-dot.connected {
        /* filled by inline style */
    }
    .port-dot:hover {
        transform: scale(1.3);
    }
    .port-label {
        font-size: 9px;
        color: var(--text);
        white-space: nowrap;
        cursor: default;
    }

    /* --- Inline slider (Blender-style colored bar) --- */
    .port-slider {
        position: relative;
        flex: 1;
        height: 14px;
        background: color-mix(in srgb, var(--text) 8%, transparent);
        border-radius: 3px;
        overflow: hidden;
        cursor: ew-resize;
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 0 4px;
    }
    .port-slider-fill {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        opacity: 0.3;
        border-radius: 3px;
        pointer-events: none;
    }
    .port-slider-label {
        font-size: 8px;
        color: var(--text);
        position: relative;
        pointer-events: none;
        white-space: nowrap;
    }
    .port-slider-value {
        font-size: 8px;
        color: var(--text);
        position: relative;
        pointer-events: none;
        white-space: nowrap;
        opacity: 0.7;
    }
    .port-slider-edit {
        flex: 1;
        height: 14px;
        border: 1px solid var(--accent);
        border-radius: 3px;
        background: var(--bg);
        color: var(--text);
        font-size: 9px;
        padding: 0 4px;
        outline: none;
        font-family: inherit;
    }
</style>
