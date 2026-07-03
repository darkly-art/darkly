<script lang="ts">
    import { setContext } from 'svelte';
    import { app } from '../../state/app.svelte';
    import CurveEditor from '../CurveEditor.svelte';
    import LevelsEditor from './LevelsEditor.svelte';
    import { createGraphCoords } from '../brush_builder/coords';
    import type { NodeCanvasContext } from '../brush_builder/NodeCanvas.svelte';
    import {
        partitionFilterParams,
        channelLabel,
        type FilterParam,
        type CurvePoints,
        type LevelsValues,
    } from './filterParams';
    import Slider from '../settings/widgets/Slider.svelte';

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
    const channels = $derived(partition.channels);
    const scalars = $derived(partition.scalars);

    // The channel currently shown in the per-channel editor. Kept valid as the
    // param set changes (layer swap, load) — defaults to the first channel.
    let selectedChannel = $state<string | null>(null);
    $effect(() => {
        if (channels.length === 0) {
            selectedChannel = null;
        } else if (!channels.some((c) => c.name === selectedChannel)) {
            selectedChannel = channels[0].name;
        }
    });
    const selectedParam = $derived(channels.find((c) => c.name === selectedChannel) ?? null);

    // --- Input histogram (Levels only) ---------------------------------------
    // Channel name → its row in the engine's 8×256 histogram buffer (same order
    // the LUT filter and the histogram compute shader use).
    const HISTOGRAM_CHANNEL: Record<string, number> = {
        rgb: 0, red: 1, green: 2, blue: 3, alpha: 4, hue: 5, saturation: 6, lightness: 7,
    };
    const HIST_BINS = 256;
    const showsLevels = $derived(channels.some((c) => c.kind === 'levels'));
    // Stable primitive id: a layer-tree refresh replaces the `node` object but
    // keeps the same id, so keying the effect on this (not `node.id`) stops it
    // re-running — and re-fetching the histogram — on every param edit.
    const filterId = $derived(node.id);

    let histogramBins = $state<Uint32Array | null>(null);
    // The selected channel's 256-bin slice, handed to the Levels editor.
    const selectedHistogram = $derived.by(() => {
        if (!histogramBins || !selectedParam) return null;
        const idx = HISTOGRAM_CHANNEL[selectedParam.name] ?? 0;
        return Array.from(histogramBins.subarray(idx * HIST_BINS, (idx + 1) * HIST_BINS));
    });

    // While a Levels filter is shown, target its input for histogram compute
    // and poll for the result. Cleared when the panel changes or unmounts.
    $effect(() => {
        const engine = app.engine;
        const id = filterId;
        if (!engine || !showsLevels) return;
        engine.post('request_histogram', { id });
        let stopped = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        const poll = () => {
            if (stopped) return;
            engine
                .send<{ bytes: Uint8Array }>('histogram_result', { id })
                .then(({ bytes }) => {
                    if (stopped) return;
                    if (bytes && bytes.length >= 8 * HIST_BINS * 4) {
                        // Copy to an aligned buffer before viewing as u32.
                        histogramBins = new Uint32Array(bytes.slice().buffer);
                    }
                    timer = setTimeout(poll, 500);
                })
                .catch(() => {
                    if (!stopped) timer = setTimeout(poll, 500);
                });
        };
        poll();
        return () => {
            stopped = true;
            if (timer !== undefined) clearTimeout(timer);
            histogramBins = null;
            engine.post('request_histogram', { id: -1 });
        };
    });

    // Post the whole `{name: value}` param map. `refresh` re-pulls the layer
    // tree (skipped mid curve-drag so the live edit isn't churned; the editor
    // holds its own drag state either way).
    function pushParams(refresh: boolean) {
        if (!app.engine) return;
        const params: Record<string, number | boolean | CurvePoints> = {};
        for (const p of node.params) {
            params[p.name] = (p.value ?? p.default) as number | boolean | CurvePoints;
        }
        app.engine.api.updateFilterParams({ id: node.id, params });
        if (refresh) app.refreshLayerTree();
        app.requestFrame();
    }

    function onSliderChange(param: FilterParam, v: number) {
        param.value = v;
        pushParams(true);
    }
    function onBoolChange(param: FilterParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        pushParams(true);
    }
    // Live edit (mid-drag) — post without refreshing the layer tree so the edit
    // isn't churned. Works for both the curve and levels channel editors.
    function onChannelInput(value: CurvePoints | LevelsValues) {
        if (!selectedParam) return;
        selectedParam.value = value;
        pushParams(false);
    }
    function onChannelChange(value: CurvePoints | LevelsValues) {
        if (!selectedParam) return;
        selectedParam.value = value;
        pushParams(true);
    }

    // Reset the currently-shown channel back to its schema default (the identity
    // curve for Curves, the identity transfer for Levels).
    function resetSelectedChannel() {
        if (!selectedParam) return;
        // `default` is a reactive proxy (curve pairs, or a levels array) that
        // `structuredClone` can't clone — deep-copy the array by hand.
        const d = selectedParam.default;
        selectedParam.value = (
            Array.isArray(d) ? d.map((v) => (Array.isArray(v) ? [...v] : v)) : d
        ) as typeof d;
        pushParams(true);
    }
</script>

<div class="filter-props" bind:this={rootEl}>
    <div class="header">
        <span class="type-label">{filterLabel}</span>
    </div>

    {#if channels.length > 0}
        {#if channels.length > 1}
            <div class="row">
                <span class="label">Channel</span>
                <select class="channel-select" bind:value={selectedChannel}>
                    {#each channels as c (c.name)}
                        <option value={c.name}>{channelLabel(c.name)}</option>
                    {/each}
                </select>
            </div>
        {/if}
        {#if selectedParam}
            <!-- Remount the editor on channel switch so its drag/selection
                 state resets to the newly-shown channel. -->
            {#key selectedChannel}
                {#if selectedParam.kind === 'levels'}
                    <LevelsEditor
                        values={(selectedParam.value ?? selectedParam.default) as LevelsValues}
                        histogram={selectedHistogram}
                        oninput={onChannelInput}
                        onchange={onChannelChange}
                    />
                {:else}
                    <CurveEditor
                        points={(selectedParam.value ?? selectedParam.default) as CurvePoints}
                        oninput={onChannelInput}
                        onchange={onChannelChange}
                    />
                {/if}
            {/key}
            <button
                class="reset-btn"
                onclick={resetSelectedChannel}
                title="Reset {channelLabel(selectedParam.name)} channel"
            >
                Reset
            </button>
        {/if}
    {/if}

    {#each scalars as param (param.name)}
        <div class="row">
            <span class="label">{param.name}</span>
            {#if param.kind === 'float' || param.kind === 'int'}
                <Slider
                    value={(param.value ?? param.default) as number}
                    min={(param.min ?? 0) as number}
                    max={(param.max ?? 1) as number}
                    integer={param.kind === 'int'}
                    onchange={(v) => onSliderChange(param, v)}
                    format={(v) => (param.kind === 'int' ? String(v) : v.toFixed(2))}
                />
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
