<script lang="ts">
    import { brushGraph, type NodeInstance, type PortDef } from '../../state/brush_graph.svelte';
    import { app } from '../../state/app.svelte';
    import PortWidget from './PortWidget.svelte';
    import CurveEditor from '../CurveEditor.svelte';

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
        return !!t.closest('.port-dot, .port-slider, .param-scrub, .curve-editor, .param-text-input, input, button, select');
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

    /** Commit param to Rust on checkbox change. */
    function onParamChange(index: number, e: Event) {
        const target = e.target as HTMLInputElement;
        const def = paramDefs[index] as any;
        if (!def) return;
        if (def.kind === 'bool') {
            brushGraph.setParam(node.id, index, def.kind, target.checked);
        }
    }

    // --- Param scrub (Blender-style drag bar for float/int params) ---

    let scrubIndex = -1;
    let scrubEl: HTMLDivElement | null = null;

    function paramScrubFraction(e: PointerEvent): number {
        if (!scrubEl) return 0;
        const rect = scrubEl.getBoundingClientRect();
        return Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    }

    function paramValueFromFraction(frac: number, def: any): number {
        const raw = def.min + frac * (def.max - def.min);
        return def.kind === 'int' ? Math.round(raw) : raw;
    }

    function paramScrubPercent(index: number): number {
        const def = paramDefs[index] as any;
        if (!def || def.max <= def.min) return 0;
        const val = node.params[index] ?? def.default;
        return ((val - def.min) / (def.max - def.min)) * 100;
    }

    function paramDisplayValue(index: number): string {
        const def = paramDefs[index] as any;
        if (!def) return '';
        const val = node.params[index] ?? def.default;
        return def.kind === 'int' ? String(Math.round(val)) : val.toFixed(2);
    }

    function onParamScrubDown(e: PointerEvent, index: number) {
        e.stopPropagation();
        e.preventDefault();
        scrubIndex = index;
        scrubEl = e.currentTarget as HTMLDivElement;
        scrubEl.setPointerCapture(e.pointerId);
        app.beginInteraction();
        const def = paramDefs[index] as any;
        const value = paramValueFromFraction(paramScrubFraction(e), def);
        brushGraph.setParamLocal(node.id, index, value);
    }

    function onParamScrubMove(e: PointerEvent, index: number) {
        if (scrubIndex !== index) return;
        const def = paramDefs[index] as any;
        const value = paramValueFromFraction(paramScrubFraction(e), def);
        brushGraph.setParamLocal(node.id, index, value);
    }

    function onParamScrubUp(e: PointerEvent, index: number) {
        if (scrubIndex !== index) return;
        scrubIndex = -1;
        const el = e.currentTarget as HTMLDivElement;
        el.releasePointerCapture(e.pointerId);
        const def = paramDefs[index] as any;
        const value = node.params[index] ?? def.default;
        brushGraph.setParam(node.id, index, def.kind, value);
    }

    function onParamScrubLostCapture() {
        scrubIndex = -1;
        scrubEl = null;
        app.endInteraction();
    }

    // --- Double-click to type a param value ---
    let editingParam = $state(-1);

    function onParamDblClick(e: MouseEvent, index: number) {
        e.stopPropagation();
        e.preventDefault();
        editingParam = index;
    }

    function onParamEditKeyDown(e: KeyboardEvent, index: number) {
        if (e.key === 'Enter') commitParamEdit(e.currentTarget as HTMLInputElement, index);
        if (e.key === 'Escape') editingParam = -1;
    }

    function onParamEditBlur(e: FocusEvent, index: number) {
        commitParamEdit(e.currentTarget as HTMLInputElement, index);
    }

    function commitParamEdit(input: HTMLInputElement, index: number) {
        editingParam = -1;
        const def = paramDefs[index] as any;
        if (!def) return;
        const parsed = parseFloat(input.value);
        if (isNaN(parsed)) return;
        const clamped = Math.max(def.min, Math.min(def.max, parsed));
        const value = def.kind === 'int' ? Math.round(clamped) : clamped;
        brushGraph.setParamLocal(node.id, index, value);
        brushGraph.setParam(node.id, index, def.kind, value);
    }

    // --- Enum dropdown ---

    function onEnumChange(index: number, e: Event) {
        e.stopPropagation();
        const value = parseInt((e.target as HTMLSelectElement).value);
        brushGraph.setParamLocal(node.id, index, value);
        brushGraph.setParam(node.id, index, 'int', value);
    }

    // --- Icon picker (custom dropdown) ---

    let iconPickerOpen = $state(-1);

    function toggleIconPicker(e: MouseEvent, index: number) {
        e.stopPropagation();
        iconPickerOpen = iconPickerOpen === index ? -1 : index;
    }

    function selectIcon(index: number, value: string) {
        iconPickerOpen = -1;
        brushGraph.setParamLocal(node.id, index, value);
        brushGraph.setParam(node.id, index, 'string', value);
    }

    // --- String / FloatInput text fields ---

    function onStringCommit(index: number, e: Event) {
        const value = (e.target as HTMLInputElement).value;
        brushGraph.setParamLocal(node.id, index, value);
        brushGraph.setParam(node.id, index, 'string', value);
    }

    function onFloatInputCommit(index: number, e: Event) {
        const def = paramDefs[index] as any;
        if (!def) return;
        const parsed = parseFloat((e.target as HTMLInputElement).value);
        if (isNaN(parsed)) return;
        const clamped = Math.max(def.min, Math.min(def.max, parsed));
        brushGraph.setParamLocal(node.id, index, clamped);
        brushGraph.setParam(node.id, index, 'float', clamped);
    }

    function onTextKeyDown(e: KeyboardEvent, index: number, kind: string) {
        if (e.key === 'Enter') {
            (e.target as HTMLInputElement).blur();
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
                    {#if pdef.kind === 'curve'}
                        <CurveEditor
                            points={node.params[i] ?? pdef.default}
                            oninput={(pts) => brushGraph.setParamLocal(node.id, i, pts)}
                            onchange={(pts) => brushGraph.setParam(node.id, i, 'curve', JSON.stringify(pts))}
                        />
                    {:else if pdef.kind === 'bool'}
                        <div class="param-row">
                            <span class="param-label">{pdef.name}</span>
                            <input
                                type="checkbox"
                                checked={node.params[i]}
                                onchange={(e) => onParamChange(i, e)}
                            />
                        </div>
                    {:else if pdef.kind === 'float' || pdef.kind === 'int'}
                        {#if editingParam === i}
                            <!-- svelte-ignore a11y_autofocus -->
                            <input
                                class="param-scrub-edit"
                                type="text"
                                value={node.params[i] ?? pdef.default}
                                autofocus
                                onkeydown={(e) => onParamEditKeyDown(e, i)}
                                onblur={(e) => onParamEditBlur(e, i)}
                                onclick={(e) => e.stopPropagation()}
                            />
                        {:else}
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <div
                                class="param-scrub"
                                onpointerdown={(e) => onParamScrubDown(e, i)}
                                onpointermove={(e) => onParamScrubMove(e, i)}
                                onpointerup={(e) => onParamScrubUp(e, i)}
                                onlostpointercapture={onParamScrubLostCapture}
                                ondblclick={(e) => onParamDblClick(e, i)}
                            >
                                <div
                                    class="param-scrub-fill"
                                    style="width: {paramScrubPercent(i)}%;"
                                ></div>
                                <span class="param-scrub-label">{pdef.name}</span>
                                <span class="param-scrub-value">{paramDisplayValue(i)}</span>
                            </div>
                        {/if}
                    {:else if pdef.kind === 'enum'}
                        <div class="param-row">
                            <span class="param-label">{pdef.name}</span>
                            <select
                                class="param-select"
                                value={node.params[i] ?? pdef.default}
                                onchange={(e) => onEnumChange(i, e)}
                                onclick={(e) => e.stopPropagation()}
                            >
                                {#each pdef.options as option, oi}
                                    <option value={oi}>{option}</option>
                                {/each}
                            </select>
                        </div>
                    {:else if pdef.kind === 'string'}
                        <div class="param-row">
                            <span class="param-label">{pdef.name}</span>
                            <input
                                class="param-text-input"
                                type="text"
                                value={node.params[i] ?? pdef.default}
                                onblur={(e) => onStringCommit(i, e)}
                                onkeydown={(e) => onTextKeyDown(e, i, 'string')}
                                onclick={(e) => e.stopPropagation()}
                            />
                        </div>
                    {:else if pdef.kind === 'floatInput'}
                        <div class="param-row">
                            <span class="param-label">{pdef.name}</span>
                            <input
                                class="param-text-input"
                                type="text"
                                value={node.params[i] ?? pdef.default}
                                onblur={(e) => onFloatInputCommit(i, e)}
                                onkeydown={(e) => onTextKeyDown(e, i, 'float')}
                                onclick={(e) => e.stopPropagation()}
                            />
                        </div>
                    {:else if pdef.kind === 'icon'}
                        <div class="param-row icon-picker-row">
                            <span class="param-label">{pdef.name}</span>
                            <button
                                class="icon-picker-trigger"
                                onclick={(e) => toggleIconPicker(e, i)}
                            >
                                {#if node.params[i]}
                                    <i class="{node.params[i]} icon-picker-current"></i>
                                {:else}
                                    <span class="icon-picker-none">None</span>
                                {/if}
                                <svg class="chevron" width="8" height="5" viewBox="0 0 10 6">
                                    <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" fill="none"/>
                                </svg>
                            </button>
                            {#if iconPickerOpen === i}
                                <div class="icon-picker-dropdown dropdown-surface">
                                    {#each pdef.options as [iconClass, iconLabel]}
                                        <button
                                            class="icon-picker-item"
                                            class:active={(node.params[i] ?? pdef.default) === iconClass}
                                            onclick={(e) => { e.stopPropagation(); selectIcon(i, iconClass); }}
                                        >
                                            {#if iconClass}
                                                <i class="{iconClass} icon-picker-item-icon"></i>
                                            {:else}
                                                <span class="icon-picker-item-icon" style="width:14px"></span>
                                            {/if}
                                            <span>{iconLabel}</span>
                                        </button>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    {/if}
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

    /* --- Param scrub bar (Blender-style) --- */
    .param-scrub {
        position: relative;
        height: 14px;
        background: color-mix(in srgb, var(--text) 8%, transparent);
        border-radius: 3px;
        overflow: hidden;
        cursor: ew-resize;
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 0 4px;
        margin-top: 2px;
    }
    .param-scrub-fill {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        background: var(--accent);
        opacity: 0.3;
        border-radius: 3px;
        pointer-events: none;
    }
    .param-scrub-label {
        font-size: 8px;
        color: var(--text);
        position: relative;
        pointer-events: none;
        white-space: nowrap;
    }
    .param-scrub-value {
        font-size: 8px;
        color: var(--text);
        position: relative;
        pointer-events: none;
        white-space: nowrap;
        opacity: 0.7;
    }
    .param-scrub-edit {
        height: 14px;
        border: 1px solid var(--accent);
        border-radius: 3px;
        background: var(--bg);
        color: var(--text);
        font-size: 9px;
        padding: 0 4px;
        outline: none;
        font-family: inherit;
        margin-top: 2px;
        width: 100%;
        box-sizing: border-box;
    }

    /* --- Text input for string/floatInput params --- */
    .param-text-input {
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
    .param-text-input:focus {
        border-color: var(--accent);
    }

    /* --- Enum dropdown & Icon picker --- */
    .param-select {
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
    }
    .param-select:focus {
        border-color: var(--accent);
    }
    /* --- Icon picker --- */
    .icon-picker-row {
        position: relative;
    }
    .icon-picker-trigger {
        flex: 1;
        display: flex;
        align-items: center;
        gap: 4px;
        height: 16px;
        border: 1px solid color-mix(in srgb, var(--text) 20%, transparent);
        border-radius: 3px;
        background: var(--bg);
        color: var(--text);
        font-size: 9px;
        padding: 0 4px;
        cursor: pointer;
        font-family: inherit;
    }
    .icon-picker-trigger:hover {
        border-color: var(--accent);
    }
    :global(.icon-picker-current) {
        font-size: 10px;
    }
    .icon-picker-none {
        opacity: 0.5;
        font-size: 8px;
    }
    .icon-picker-trigger .chevron {
        margin-left: auto;
        color: var(--text-muted);
        flex-shrink: 0;
    }
    .icon-picker-dropdown {
        position: absolute;
        top: 100%;
        left: 0;
        right: 0;
        min-width: 120px;
        max-height: 160px;
        overflow-y: auto;
        z-index: 100;
        padding: 2px 0;
        background: var(--bg-raised);
        border: 1px solid color-mix(in srgb, var(--text) 15%, transparent);
        border-radius: 4px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.3);
    }
    .icon-picker-item {
        display: flex;
        align-items: center;
        gap: 6px;
        width: 100%;
        border: none;
        background: none;
        color: var(--text);
        font-size: 9px;
        padding: 3px 6px;
        cursor: pointer;
        font-family: inherit;
        text-align: left;
    }
    .icon-picker-item:hover {
        background: var(--bg-hover);
    }
    .icon-picker-item.active {
        color: var(--accent);
    }
    :global(.icon-picker-item-icon) {
        font-size: 11px;
        width: 14px;
        text-align: center;
        flex-shrink: 0;
    }
</style>
