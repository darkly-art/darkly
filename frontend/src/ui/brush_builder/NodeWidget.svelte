<script lang="ts">
    import { brushGraph, type NodeInstance, type PortDef } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
    import PortWidget from './PortWidget.svelte';

    interface Props {
        node: NodeInstance;
        zoom: number;
    }

    let { node, zoom }: Props = $props();

    let isSelected = $derived(brushGraph.selectedNode === node.id);
    let inputPorts = $derived(node.ports.filter(p => p.dir === 'Input'));
    let outputPorts = $derived(node.ports.filter(p => p.dir === 'Output'));

    // Node type info for display name and params.
    let typeInfo = $derived(brushGraph.getNodeType(node.type_id));
    let displayName = $derived(typeInfo?.display_name ?? node.type_id);
    let paramDefs = $derived(typeInfo?.params ?? []);

    // --- Drag to move (from any point on the node) ---
    let dragging = false;
    let dragStartX = 0;
    let dragStartY = 0;
    let nodeStartX = 0;
    let nodeStartY = 0;
    let nodeEl: HTMLDivElement;

    /** Returns true if the event target is an interactive child that should
     *  handle its own pointer events (port dots, sliders, buttons). */
    function isInteractiveTarget(e: PointerEvent): boolean {
        const t = e.target as HTMLElement;
        return !!t.closest('.port-dot, .port-slider, input, button');
    }

    function onNodeDown(e: PointerEvent) {
        if (isInteractiveTarget(e)) return;
        e.stopPropagation();
        brushGraph.selectedNode = node.id;
        dragging = true;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        nodeStartX = node.position[0];
        nodeStartY = node.position[1];
        nodeEl.setPointerCapture(e.pointerId);
        app.beginInteraction();
    }

    function onNodeMove(e: PointerEvent) {
        if (!dragging) return;
        const dx = (e.clientX - dragStartX) / zoom;
        const dy = (e.clientY - dragStartY) / zoom;
        brushGraph.moveNode(node.id, nodeStartX + dx, nodeStartY + dy);
    }

    function onNodeUp(e: PointerEvent) {
        if (!dragging) return;
        dragging = false;
        nodeEl.releasePointerCapture(e.pointerId);
        brushGraph.syncNodePosition(node.id);
    }

    /** Guaranteed cleanup — fires when capture ends for any reason. */
    function onNodeLostCapture() {
        dragging = false;
        app.endInteraction();
    }

    /** Local update for responsive slider feedback. */
    function onParamInput(index: number, e: Event) {
        const target = e.target as HTMLInputElement;
        const def = paramDefs[index] as any;
        if (def?.kind === 'bool') {
            brushGraph.setParamLocal(node.id, index, target.checked);
        } else if (def?.kind === 'float') {
            brushGraph.setParamLocal(node.id, index, parseFloat(target.value));
        } else if (def?.kind === 'int') {
            brushGraph.setParamLocal(node.id, index, parseInt(target.value));
        }
    }

    /** Commit param to Rust on slider release / checkbox change. */
    function onParamChange(index: number, e: Event) {
        const target = e.target as HTMLInputElement;
        const def = paramDefs[index] as any;
        if (!def) return;
        if (def.kind === 'bool') {
            brushGraph.setParam(node.id, index, def.kind, target.checked);
        } else if (def.kind === 'float') {
            brushGraph.setParam(node.id, index, def.kind, parseFloat(target.value));
        } else if (def.kind === 'int') {
            brushGraph.setParam(node.id, index, def.kind, parseInt(target.value));
        }
    }

    function onRemove(e: MouseEvent) {
        e.stopPropagation();
        brushGraph.removeNode(node.id);
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="node-widget"
    class:selected={isSelected}
    style="transform: translate({node.position[0]}px, {node.position[1]}px);"
    data-node-id={node.id}
    bind:this={nodeEl}
    onpointerdown={onNodeDown}
    onpointermove={onNodeMove}
    onpointerup={onNodeUp}
    onlostpointercapture={onNodeLostCapture}
>
    <div class="node-header">
        <span class="node-title">{displayName}</span>
        <button class="remove-btn" onclick={onRemove} title="Remove node">&times;</button>
    </div>

    <div class="node-body">
        {#if outputPorts.length > 0}
            <div class="ports-outputs">
                {#each outputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="right" />
                {/each}
            </div>
        {/if}
        {#if inputPorts.length > 0}
            <div class="ports-inputs">
                {#each inputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="left" />
                {/each}
            </div>
        {/if}

        {#if paramDefs.length > 0}
            <div class="params">
                {#each paramDefs as pdef, i}
                    <div class="param-row">
                        <span class="param-label">{pdef.name}</span>
                        {#if pdef.kind === 'bool'}
                            <input
                                type="checkbox"
                                checked={node.params[i]}
                                onchange={(e) => onParamChange(i, e)}
                            />
                        {:else if pdef.kind === 'float'}
                            <input
                                type="range"
                                class="param-slider"
                                min={pdef.min}
                                max={pdef.max}
                                step={((pdef.max - pdef.min) / 100)}
                                value={node.params[i] ?? pdef.default}
                                oninput={(e) => onParamInput(i, e)}
                                onchange={(e) => onParamChange(i, e)}
                            />
                            <span class="param-value">{(node.params[i] ?? pdef.default).toFixed(2)}</span>
                        {:else if pdef.kind === 'int'}
                            <input
                                type="range"
                                class="param-slider"
                                min={pdef.min}
                                max={pdef.max}
                                step="1"
                                value={node.params[i] ?? pdef.default}
                                oninput={(e) => onParamInput(i, e)}
                                onchange={(e) => onParamChange(i, e)}
                            />
                            <span class="param-value">{node.params[i] ?? pdef.default}</span>
                        {/if}
                    </div>
                {/each}
            </div>
        {/if}
    </div>
</div>

<style>
    .node-widget {
        position: absolute;
        left: 0;
        top: 0;
        min-width: 140px;
        background: var(--bg-active);
        border: 1px solid color-mix(in srgb, var(--text) 15%, transparent);
        border-radius: 6px;
        font-size: 11px;
        cursor: grab;
        user-select: none;
    }
    .node-widget:active {
        cursor: grabbing;
    }
    .node-widget.selected {
        border-color: var(--accent);
    }
    .node-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 4px 6px;
        background: var(--bg);
        border-radius: 5px 5px 0 0;
    }
    .node-title {
        font-weight: 600;
        color: var(--text);
        font-size: 10px;
    }
    .remove-btn {
        background: none;
        border: none;
        color: var(--text);
        cursor: pointer;
        font-size: 14px;
        padding: 0 2px;
        line-height: 1;
        transition: color 0.1s;
    }
    .remove-btn:hover {
        color: var(--danger);
    }
    .node-body {
        padding: 4px 0;
    }
    .ports-outputs {
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 2px;
        padding: 0 2px;
    }
    .ports-inputs {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 0 2px;
    }
    .params {
        padding: 4px 6px;
        border-top: 1px solid color-mix(in srgb, var(--text) 8%, transparent);
        margin-top: 4px;
    }
    .param-row {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-top: 2px;
    }
    .param-label {
        font-size: 9px;
        color: var(--text);
        cursor: default;
    }
    .param-slider {
        flex: 1;
        height: 3px;
    }
    .param-slider::-webkit-slider-thumb {
        width: 8px;
        height: 8px;
    }
    .param-value {
        font-size: 8px;
        color: var(--text);
        text-align: right;
        cursor: default;
    }
</style>
