<script lang="ts">
    import { brushGraph, type NodeInstance, type PortDef } from '../../state/brush_graph.svelte';
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

    // --- Drag to move ---
    let dragging = false;
    let dragStartX = 0;
    let dragStartY = 0;
    let nodeStartX = 0;
    let nodeStartY = 0;

    function onHeaderDown(e: PointerEvent) {
        e.stopPropagation();
        brushGraph.selectedNode = node.id;
        dragging = true;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        nodeStartX = node.position[0];
        nodeStartY = node.position[1];
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
    }

    function onHeaderMove(e: PointerEvent) {
        if (!dragging) return;
        const dx = (e.clientX - dragStartX) / zoom;
        const dy = (e.clientY - dragStartY) / zoom;
        brushGraph.moveNode(node.id, nodeStartX + dx, nodeStartY + dy);
    }

    function onHeaderUp(e: PointerEvent) {
        dragging = false;
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
        // Sync final position to Rust.
        brushGraph.syncNodePosition(node.id);
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

<div
    class="node-widget"
    class:selected={isSelected}
    style="transform: translate({node.position[0]}px, {node.position[1]}px);"
    data-node-id={node.id}
>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="node-header"
        onpointerdown={onHeaderDown}
        onpointermove={onHeaderMove}
        onpointerup={onHeaderUp}
    >
        <span class="node-title">{displayName}</span>
        <button class="remove-btn" onclick={onRemove} title="Remove node">&times;</button>
    </div>

    <div class="node-body">
        <div class="ports-columns">
            <div class="ports-left">
                {#each inputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="left" />
                {/each}
            </div>
            <div class="ports-right">
                {#each outputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="right" />
                {/each}
            </div>
        </div>

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
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 6px;
        font-size: 11px;
        box-shadow: 0 2px 8px rgba(0,0,0,0.4);
    }
    .node-widget.selected {
        border-color: #6a6aff;
    }
    .node-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 4px 6px;
        background: #333;
        border-radius: 5px 5px 0 0;
        cursor: grab;
        user-select: none;
    }
    .node-header:active {
        cursor: grabbing;
    }
    .node-title {
        font-weight: 600;
        color: #ddd;
        font-size: 10px;
    }
    .remove-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        font-size: 14px;
        padding: 0 2px;
        line-height: 1;
    }
    .remove-btn:hover {
        color: #ff6b6b;
    }
    .node-body {
        padding: 4px 0;
    }
    .ports-columns {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        padding: 0 2px;
    }
    .ports-left, .ports-right {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }
    .params {
        padding: 4px 6px;
        border-top: 1px solid #3a3a3a;
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
        color: #999;
        min-width: 40px;
    }
    .param-slider {
        flex: 1;
        height: 3px;
        -webkit-appearance: none;
        appearance: none;
        background: #444;
        border-radius: 2px;
        outline: none;
        cursor: pointer;
    }
    .param-slider::-webkit-slider-thumb {
        -webkit-appearance: none;
        width: 8px;
        height: 8px;
        border-radius: 50%;
        background: #6a6aff;
        cursor: pointer;
    }
    .param-value {
        font-size: 8px;
        color: #888;
        min-width: 28px;
        text-align: right;
    }
</style>
