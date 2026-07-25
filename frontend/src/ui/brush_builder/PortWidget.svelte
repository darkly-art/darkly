<script lang="ts">
    import { getContext, untrack } from 'svelte';
    import { brushGraph, WIRE_COLORS, type PortDef } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
    import type { NodeCanvasContext } from './NodeCanvas.svelte';
    import Icon from '../../icons/Icon.svelte';
    import CurveEditor from '../CurveEditor.svelte';

    interface Props {
        nodeId: string;
        port: PortDef;
        side: 'left' | 'right';
    }

    let { nodeId, port, side }: Props = $props();

    /** Canonical port definition from the node type registration.
     *  Display metadata (unit_type, icon, label, description) comes from
     *  here — not the instance — so it stays current even for old graphs. */
    let regPort = $derived.by(() => {
        const node = brushGraph.graph?.nodes[nodeId];
        if (!node) return null;
        const nodeType = brushGraph.getNodeType(node.type_id);
        return nodeType?.ports.find(p => p.name === port.name && p.dir === port.dir) ?? null;
    });

    let color = $derived(WIRE_COLORS[port.wire_type] ?? '#888');
    let connected = $derived(brushGraph.isPortConnected(nodeId, port.name, port.dir));

    /** Numeric view of the input's value for the slider path (Scalar/Int/Bool).
     *  A bool value may arrive as a real boolean or a number. */
    let numValue = $derived(
        typeof port.value === 'number'
            ? port.value
            : (typeof port.value === 'boolean' ? (port.value ? 1 : 0) : 0)
    );

    /** Wire-type sets driving the widget branch for disconnected inputs. */
    const SLIDER_TYPES = new Set(['Scalar', 'Int', 'Bool']);
    let isInput = $derived(port.dir === 'Input');
    let showSlider = $derived(isInput && !connected && SLIDER_TYPES.has(port.wire_type));
    let showEnum = $derived(isInput && !connected && port.wire_type === 'Enum');
    let showString = $derived(isInput && !connected && port.wire_type === 'String');
    let showCurve = $derived(isInput && !connected && port.wire_type === 'Curve');

    /** A wire endpoint gets a connector dot: outputs always, inputs only when
     *  the wire type is wirable (non-wirable inputs like Enum/String/Curve
     *  show their widget + expose button but no dot). */
    let showDot = $derived(port.dir === 'Output' || port.wirable);

    /** A settable-source input (`port.source`) also offers a right-edge source
     *  handle to wire *from* — but only while the input itself is undriven. Once
     *  a wire drives the port its value is the driver's, so the source handle
     *  hides (mirror of `showSlider` hiding the value widget when connected). */
    let showSourceHandle = $derived(port.dir === 'Input' && port.source && !connected);

    /** Commit kind for the slider path, keyed off the wire type. */
    let sliderKind = $derived(
        port.wire_type === 'Int' ? 'int' : port.wire_type === 'Bool' ? 'bool' : 'float'
    );

    // --- Port offset registration ---
    const { register, unregister, coords } = getContext<NodeCanvasContext>('node-canvas');
    // Non-reactive ref: bind:this re-runs on every render, and reading a
    // reactive dotEl from within $effect would re-fire the effect, which
    // mutates portVersion → re-render → rebind → infinite loop.
    let dotEl: HTMLDivElement | undefined;

    // Re-register whenever `port.name` changes. The `{#each inputPorts}` in
    // NodeWidget isn't keyed, so when `visible_when` hides/shows a sibling
    // port, Svelte rebinds existing instances by index — every reused
    // instance from the hidden port's index downward gets a *different*
    // `port` (and a different DOM row). Tracking `port.name` therefore
    // catches every layout shift: an inserted/removed port always changes
    // `port.name` on at least every index from the change onward.
    $effect(() => {
        // Tracked reads — re-fire when the port identity changes.
        const portName = port.name;
        const portDir = port.dir;
        // The measurement + side-effect are wrapped in `untrack` so the
        // `zoom` read inside `coords.elementCenterInParent` and the
        // `portVersion` mutation inside `register` don't feed back into
        // this effect's dep set (which would cause a re-render loop).
        untrack(() => {
            if (!dotEl) return;
            const nodeEl = dotEl.closest('[data-node-id]') as HTMLElement;
            if (!nodeEl) return;
            register(nodeId, portName, portDir, coords.elementCenterInParent(dotEl, nodeEl));
        });
        return () => untrack(() => unregister(nodeId, portName, portDir));
    });

    // A settable-source input carries a *second* wire endpoint on the same row:
    // an `Output`-role source handle on the right edge. Registered under the
    // `(node, name, 'Output')` key (distinct from the left `Input` dot), so the
    // wire renderer resolves wires leaving `node.name` to this offset.
    let sourceDotEl: HTMLDivElement | undefined;
    $effect(() => {
        const portName = port.name;
        const shown = showSourceHandle;
        untrack(() => {
            if (!shown || !sourceDotEl) return;
            const nodeEl = sourceDotEl.closest('[data-node-id]') as HTMLElement;
            if (!nodeEl) return;
            register(nodeId, portName, 'Output', coords.elementCenterInParent(sourceDotEl, nodeEl));
        });
        return () => untrack(() => unregister(nodeId, portName, 'Output'));
    });

    /** Start dragging a wire *from* this settable-source's source handle. */
    function onSourceDown(e: PointerEvent) {
        e.preventDefault();
        brushGraph.draggingFrom = { node: nodeId, port: port.name, dir: 'Output' };
    }

    /** Drop onto the source handle: accept a wire dragged from an input, same
     *  as dropping on a real output. */
    function onSourceUp(e: PointerEvent) {
        e.stopPropagation();
        e.preventDefault();
        const drag = brushGraph.draggingFrom;
        if (!drag) return;
        if (drag.dir === 'Input' && !(drag.node === nodeId && drag.port === port.name)) {
            brushGraph.connect(nodeId, port.name, drag.node, drag.port);
        }
        brushGraph.draggingFrom = null;
        brushGraph.dragMouse = null;
    }

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

    // --- Inline slider for disconnected Scalar/Int/Bool inputs ---

    let sliderEl = $state<HTMLDivElement>();
    let sliding = false;

    /** Normalized position (0–1) from a pointer event relative to the slider bar. */
    function sliderFraction(e: PointerEvent): number {
        if (!sliderEl) return 0;
        const local = coords.clientToElementLocal(sliderEl, e.clientX, e.clientY);
        return Math.max(0, Math.min(1, local.x / sliderEl.clientWidth));
    }

    /** Quantize a value to multiples of `port.step` from `port.min`. Returns
     *  the value unchanged when `step` is zero (continuous port). */
    function snapToStep(value: number): number {
        if (!port.step || port.step <= 0) return value;
        const stepped = Math.round((value - port.min) / port.step) * port.step + port.min;
        return Math.max(port.min, Math.min(port.max, stepped));
    }

    function valueFromFraction(frac: number): number {
        const raw = port.min + frac * (port.max - port.min);
        if (port.wire_type === 'Int') return Math.round(raw);
        if (port.wire_type === 'Bool') return frac >= 0.5 ? 1 : 0;
        return snapToStep(raw);
    }

    /** Commit a slider value, mapping Bool's numeric slider value to a real
     *  boolean and passing Int/Scalar as numbers. */
    function commitSlider(value: number) {
        const committed = port.wire_type === 'Bool' ? Number(value) >= 0.5 : value;
        brushGraph.setInput(nodeId, port.name, sliderKind, committed);
    }

    function onSliderDown(e: PointerEvent) {
        if (!sliderEl) return;
        // Stop propagation so the node doesn't start dragging.
        e.stopPropagation();
        e.preventDefault();
        sliding = true;
        sliderEl.setPointerCapture(e.pointerId);
        app.beginInteraction();
        const value = valueFromFraction(sliderFraction(e));
        brushGraph.setInputLocal(nodeId, port.name, value);
    }

    function onSliderMove(e: PointerEvent) {
        if (!sliding) return;
        const value = valueFromFraction(sliderFraction(e));
        brushGraph.setInputLocal(nodeId, port.name, value);
    }

    function onSliderUp(e: PointerEvent) {
        if (!sliding || !sliderEl) return;
        sliding = false;
        sliderEl.releasePointerCapture(e.pointerId);
        commitSlider(numValue);
    }

    function onSliderLostCapture() {
        sliding = false;
        app.endInteraction();
    }

    // --- Enum dropdown ---

    function onEnumChange(e: Event) {
        e.stopPropagation();
        const idx = parseInt((e.target as HTMLSelectElement).value);
        brushGraph.setInputLocal(nodeId, port.name, idx);
        brushGraph.setInput(nodeId, port.name, 'enum', idx);
    }

    // --- String text field ---

    function onStringCommit(e: Event) {
        const value = (e.target as HTMLInputElement).value;
        brushGraph.setInputLocal(nodeId, port.name, value);
        brushGraph.setInput(nodeId, port.name, 'string', value);
    }

    function onStringKeyDown(e: KeyboardEvent) {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
    }

    /** Every disconnected input is exposable to the brush bar. */
    let canExpose = $derived(port.dir === 'Input' && !connected);
    let isExposed = $derived(brushGraph.isPortExposed(nodeId, port.name));

    function toggleExposed(e: MouseEvent) {
        e.stopPropagation();
        if (isExposed) {
            brushGraph.unexposePort(nodeId, port.name);
        } else {
            brushGraph.exposePort(nodeId, port.name);
        }
    }

    let sliderPercent = $derived(
        port.max > port.min
            ? ((numValue - port.min) / (port.max - port.min)) * 100
            : 0
    );

    /** Convert a port-space value to display using unit_type from the registration. */
    function toDisplay(value: number): number {
        switch (regPort?.unit_type) {
            case 'Percent': return value * 100;
            // Wire unit for angle ports is radians; display in degrees.
            case 'Degrees': return value * (180 / Math.PI);
            default: return value;
        }
    }

    /** Unit suffix string. */
    function unitSuffix(): string {
        switch (regPort?.unit_type) {
            case 'Percent': return '%';
            case 'Degrees': return '°';
            default: return '';
        }
    }

    let displayValue = $derived(
        port.wire_type === 'Bool'
            ? (numValue >= 0.5 ? 'on' : 'off')
            : port.wire_type === 'Int'
                ? String(Math.round(numValue))
                : `${Math.round(toDisplay(numValue))}${unitSuffix()}`
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
        const value = port.wire_type === 'Int' ? Math.round(clamped) : snapToStep(clamped);
        brushGraph.setInputLocal(nodeId, port.name, value);
        commitSlider(value);
    }
</script>

<div
    class="port-row"
    class:port-right={side === 'right'}
    title={regPort?.description || ''}
>
    {#if showDot}
        <div
            class="port-dot"
            class:connected
            style="background: {connected ? color : 'var(--bg-active)'}; border-color: {color};"
            role="button"
            tabindex="-1"
            onpointerdown={onPointerDown}
            onpointerup={onPointerUp}
            bind:this={dotEl}
            data-port-node={nodeId}
            data-port-name={port.name}
            data-port-dir={port.dir}
        ></div>
    {/if}
    {#if showSlider}
        {#if editing}
            <!-- svelte-ignore a11y_autofocus -->
            <input
                class="port-slider-edit"
                type="text"
                value={port.wire_type === 'Int' ? Math.round(numValue) : numValue}
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
                <span class="port-slider-label">{regPort?.label || port.name}</span>
                <span class="port-slider-value">{displayValue}</span>
            </div>
        {/if}
    {:else if showEnum}
        <span class="port-label">{regPort?.label || port.name}</span>
        <select
            class="port-select"
            value={Number(port.value)}
            onchange={onEnumChange}
            onclick={(e) => e.stopPropagation()}
        >
            {#each (port.enum_options ?? []) as option, oi}
                <option value={oi}>{option}</option>
            {/each}
        </select>
    {:else if showString}
        <span class="port-label">{regPort?.label || port.name}</span>
        <input
            class="port-text-input"
            type="text"
            value={String(port.value)}
            onblur={onStringCommit}
            onkeydown={onStringKeyDown}
            onclick={(e) => e.stopPropagation()}
        />
    {:else if showCurve}
        <CurveEditor
            points={port.value as Array<[number, number]>}
            oninput={(pts) => brushGraph.setInputLocal(nodeId, port.name, pts)}
            onchange={(pts) => brushGraph.setInput(nodeId, port.name, 'curve', JSON.stringify(pts))}
        />
    {:else}
        <span class="port-label">{regPort?.label || port.name}</span>
    {/if}
    {#if canExpose}
        <button
            class="expose-toggle"
            class:exposed={isExposed}
            title={isExposed ? 'Hide from brush bar' : 'Expose in brush bar'}
            onclick={toggleExposed}
        >
            <Icon name="fa6-solid:eye" />
        </button>
    {/if}
    {#if showSourceHandle}
        <div
            class="port-dot port-source-dot"
            title="Wire this control into another node"
            style="background: var(--bg-active); border-color: {color};"
            role="button"
            tabindex="-1"
            onpointerdown={onSourceDown}
            onpointerup={onSourceUp}
            bind:this={sourceDotEl}
            data-port-node={nodeId}
            data-port-name={port.name}
            data-port-dir="Output"
        ></div>
    {/if}
</div>

<style>
    .port-row {
        position: relative;
        display: flex;
        align-items: center;
        gap: 4px;
        height: 18px;
        padding-left: 10px;
    }
    .port-right {
        flex-direction: row-reverse;
        padding-left: 0;
        padding-right: 10px;
    }
    .port-dot {
        position: absolute;
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 2px solid;
        cursor: crosshair;
        flex-shrink: 0;
        z-index: 1;
        top: 50%;
        transform: translateY(-50%);
    }
    /* Pin dots to node edge, protruding slightly. */
    .port-row:not(.port-right) .port-dot {
        left: -5px;
    }
    .port-right .port-dot {
        right: -5px;
    }
    /* The settable-source handle always sits on the node's right (output)
       edge, even though it lives inside a left-side input row. Selector
       specificity matches the `:not(.port-right) .port-dot` rule above and
       comes later, so `left` is overridden back to the right edge. */
    .port-row:not(.port-right) .port-source-dot {
        left: auto;
        right: -5px;
    }
    .port-dot:hover {
        transform: translateY(-50%) scale(1.3);
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

    /* --- Enum dropdown --- */
    .port-select {
        flex: 1;
        height: 16px;
        border: 1px solid color-mix(in srgb, var(--text) 20%, transparent);
        border-radius: 3px;
        background: var(--bg);
        color: var(--text);
        font-size: 8px;
        padding: 0 2px;
        outline: none;
        font-family: inherit;
        cursor: pointer;
        min-width: 0;
    }
    .port-select:focus {
        border-color: var(--accent);
    }

    /* --- Text input for string inputs --- */
    .port-text-input {
        flex: 1;
        height: 16px;
        border: 1px solid color-mix(in srgb, var(--text) 20%, transparent);
        border-radius: 3px;
        background: var(--bg);
        color: var(--text);
        font-size: 9px;
        padding: 0 4px;
        outline: none;
        font-family: inherit;
        min-width: 0;
    }
    .port-text-input:focus {
        border-color: var(--accent);
    }

    /* --- Expose toggle --- */
    .expose-toggle {
        width: 14px;
        height: 14px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 3px;
        color: var(--text-dim);
        cursor: pointer;
        font-size: 8px;
        flex-shrink: 0;
        padding: 0;
        opacity: 0.5;
        transition: opacity 0.1s, color 0.1s;
    }
    .expose-toggle:hover {
        opacity: 0.8;
    }
    .expose-toggle.exposed {
        opacity: 1;
        color: var(--accent);
    }
</style>
