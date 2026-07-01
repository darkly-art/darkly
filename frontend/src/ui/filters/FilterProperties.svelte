<script lang="ts">
    import { setContext } from 'svelte';
    import { app } from '../../state/app.svelte';
    import CurveEditor from '../CurveEditor.svelte';
    import { createGraphCoords } from '../brush_builder/coords';
    import type { NodeCanvasContext } from '../brush_builder/NodeCanvas.svelte';
    import { partitionFilterParams, channelLabel, type FilterParam, type CurvePoints } from './filterParams';

    /** The identity curve — a straight diagonal from (0,0) to (1,1). */
    const IDENTITY_CURVE: CurvePoints = [
        [0, 0],
        [1, 1],
    ];

    let { node }: {
        node: { id: number; pipeline: string; params: FilterParam[] };
    } = $props();

    // `CurveEditor` reads pointer coordinates through the `node-canvas` context
    // (it normally lives inside the brush graph). This panel is neither zoomed
    // nor panned, so a trivial identity coord system suffices — only
    // `clientToElementLocal` is exercised; the port register hooks are no-ops.
    let rootEl: HTMLDivElement;
    const coords = createGraphCoords({ nodeLayerEl: () => rootEl, zoom: () => 1 });
    setContext<NodeCanvasContext>('node-canvas', {
        register() {},
        unregister() {},
        coords,
    });

    const filterLabel = $derived(app.filterDisplayName(node.pipeline));

    const partition = $derived(partitionFilterParams(node.params ?? []));
    const curves = $derived(partition.curves);
    const scalars = $derived(partition.scalars);

    // The channel currently shown in the curve editor. Kept valid as the param
    // set changes (layer swap, load) — defaults to the first curve.
    let selectedChannel = $state<string | null>(null);
    $effect(() => {
        if (curves.length === 0) {
            selectedChannel = null;
        } else if (!curves.some((c) => c.name === selectedChannel)) {
            selectedChannel = curves[0].name;
        }
    });
    const selectedCurve = $derived(curves.find((c) => c.name === selectedChannel) ?? null);

    // Post the whole `{name: value}` param map. `refresh` re-pulls the layer
    // tree (skipped mid curve-drag so the live edit isn't churned; the editor
    // holds its own drag state either way).
    function pushParams(refresh: boolean) {
        if (!app.engine) return;
        const params: Record<string, number | boolean | CurvePoints> = {};
        for (const p of node.params) {
            params[p.name] = (p.value ?? p.default) as number | boolean | CurvePoints;
        }
        app.engine.post('update_filter_params', { id: node.id, params });
        if (refresh) app.refreshLayerTree();
        app.requestFrame();
    }

    function onSliderInput(param: FilterParam, e: Event) {
        const target = e.target as HTMLInputElement;
        param.value = param.kind === 'int' ? parseInt(target.value, 10) : parseFloat(target.value);
        pushParams(true);
    }
    function onBoolChange(param: FilterParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        pushParams(true);
    }
    function onCurveInput(pts: CurvePoints) {
        if (!selectedCurve) return;
        selectedCurve.value = pts;
        pushParams(false);
    }
    function onCurveChange(pts: CurvePoints) {
        if (!selectedCurve) return;
        selectedCurve.value = pts;
        pushParams(true);
    }

    // Reset the currently-shown channel's curve back to the identity diagonal.
    function resetSelectedCurve() {
        if (!selectedCurve) return;
        selectedCurve.value = IDENTITY_CURVE.map((p) => [...p] as [number, number]);
        pushParams(true);
    }
</script>

<div class="filter-props" bind:this={rootEl}>
    <div class="header">
        <span class="type-label">{filterLabel}</span>
    </div>

    {#if curves.length > 0}
        {#if curves.length > 1}
            <div class="row">
                <span class="label">Channel</span>
                <select class="channel-select" bind:value={selectedChannel}>
                    {#each curves as c (c.name)}
                        <option value={c.name}>{channelLabel(c.name)}</option>
                    {/each}
                </select>
            </div>
        {/if}
        {#if selectedCurve}
            <!-- Remount the editor on channel switch so its drag/selection
                 state resets to the newly-shown curve. -->
            {#key selectedChannel}
                <CurveEditor
                    points={(selectedCurve.value ?? selectedCurve.default) as CurvePoints}
                    oninput={onCurveInput}
                    onchange={onCurveChange}
                />
            {/key}
            <button
                class="reset-btn"
                onclick={resetSelectedCurve}
                title="Reset {channelLabel(selectedCurve.name)} curve"
            >
                Reset
            </button>
        {/if}
    {/if}

    {#each scalars as param (param.name)}
        <div class="row">
            <span class="label">{param.name}</span>
            {#if param.kind === 'float' || param.kind === 'int'}
                <input
                    type="range"
                    class="slider"
                    min={param.min}
                    max={param.max}
                    step={param.kind === 'int' ? 1 : (((param.max as number) - (param.min as number)) / 100)}
                    value={(param.value ?? param.default) as number}
                    oninput={(e) => onSliderInput(param, e)}
                />
                <span class="value">
                    {param.kind === 'int'
                        ? (param.value ?? param.default)
                        : ((param.value ?? param.default) as number).toFixed(2)}
                </span>
            {:else if param.kind === 'bool'}
                <input
                    type="checkbox"
                    class="checkbox"
                    checked={(param.value ?? param.default) as boolean}
                    onchange={(e) => onBoolChange(param, e)}
                />
            {/if}
        </div>
    {/each}

    {#if node.params.length === 0}
        <div class="empty">No parameters</div>
    {/if}
</div>

<style>
    .filter-props {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding-bottom: 4px;
        border-bottom: 1px solid var(--bg-hover);
        margin-bottom: 2px;
    }

    .type-label {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
    }

    .reset-btn {
        align-self: center;
        padding: 3px 12px;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: var(--radius-sm);
        color: var(--text-muted);
        cursor: pointer;
        font-size: 11px;
    }
    .reset-btn:hover {
        border-color: var(--accent);
        color: var(--text);
    }

    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
    }

    .label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 56px;
        text-transform: capitalize;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .channel-select {
        flex: 1;
        min-width: 0;
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        font-size: 11px;
        padding: 2px 4px;
    }

    .slider {
        flex: 1;
        height: 4px;
        min-width: 0;
    }

    .value {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 36px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .checkbox {
        accent-color: var(--accent);
    }

    .empty {
        font-size: 12px;
        color: var(--text-dim);
        text-align: center;
        padding: 4px 0;
    }
</style>
