<script lang="ts">
    import { setContext } from 'svelte';
    import CurveEditor from '../CurveEditor.svelte';
    import LevelsEditor from './LevelsEditor.svelte';
    import EnumDropdown from '../settings/widgets/EnumDropdown.svelte';
    import Slider from '../settings/widgets/Slider.svelte';
    import { createGraphCoords } from '../brush_builder/coords';
    import type { NodeCanvasContext } from '../brush_builder/NodeCanvas.svelte';
    import {
        partitionFilterParams,
        channelLabel,
        colorizeActive,
        type FilterParam,
        type FilterParamValue,
        type CurvePoints,
        type LevelsValues,
    } from './filterParams';

    // The reusable param-editing surface for a filter's params (channel selector
    // + Curve/Levels editor + scalar rows). Both the layer properties panel and
    // the destructive-apply modal embed this: it mutates each param's `value` in
    // place and reports via `oninput` (mid-drag, no commit) / `onchange`
    // (commit). The consumer owns what a report *does* — a live
    // `updateFilterParams`, or holding scratch params for a later `applyFilter`.
    let {
        params,
        histogramBins = null,
        oninput,
        onchange,
    }: {
        params: FilterParam[];
        // Full 8×256 u32 input histogram (Levels backdrop); the selected
        // channel's slice is derived here. Null when unavailable.
        histogramBins?: Uint32Array | null;
        oninput?: () => void;
        onchange?: () => void;
    } = $props();

    // `CurveEditor` reads pointer coordinates through the `node-canvas` context
    // (it normally lives inside the brush graph). This panel is neither zoomed
    // nor panned, so a trivial identity coord system suffices.
    let rootEl: HTMLDivElement;
    const coords = createGraphCoords({ nodeLayerEl: () => rootEl, zoom: () => 1 });
    setContext<NodeCanvasContext>('node-canvas', {
        register() {},
        unregister() {},
        coords,
    });

    const partition = $derived(partitionFilterParams(params ?? []));
    const channels = $derived(partition.channels);
    const scalars = $derived(partition.scalars);

    // Colorize overrides the model selector, so disable the `model` enum while
    // it's on (Krita's `hsvadjustment` UI does the same).
    const colorizeOn = $derived(colorizeActive(scalars));

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

    // Channel name → its row in the engine's 8×256 histogram buffer (same order
    // the LUT filter and the histogram compute shader use).
    const HISTOGRAM_CHANNEL: Record<string, number> = {
        rgb: 0, red: 1, green: 2, blue: 3, alpha: 4, hue: 5, saturation: 6, lightness: 7,
    };
    const HIST_BINS = 256;
    const selectedHistogram = $derived.by(() => {
        if (!histogramBins || !selectedParam) return null;
        const idx = HISTOGRAM_CHANNEL[selectedParam.name] ?? 0;
        return Array.from(histogramBins.subarray(idx * HIST_BINS, (idx + 1) * HIST_BINS));
    });

    function onSliderChange(param: FilterParam, v: number) {
        param.value = v;
        onchange?.();
    }
    function onBoolChange(param: FilterParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        onchange?.();
    }
    function onEnumChange(param: FilterParam, key: string) {
        param.value = Number(key);
        onchange?.();
    }
    // Live edit (mid-drag) — reports without committing. Works for both editors.
    function onChannelInput(value: CurvePoints | LevelsValues) {
        if (!selectedParam) return;
        selectedParam.value = value;
        oninput?.();
    }
    function onChannelChange(value: CurvePoints | LevelsValues) {
        if (!selectedParam) return;
        selectedParam.value = value;
        onchange?.();
    }

    // Reset the currently-shown channel back to its schema default.
    function resetSelectedChannel() {
        if (!selectedParam) return;
        const d = selectedParam.default;
        selectedParam.value = (
            Array.isArray(d) ? d.map((v) => (Array.isArray(v) ? [...v] : v)) : d
        ) as FilterParamValue;
        onchange?.();
    }
</script>

<div class="filter-params" bind:this={rootEl}>
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
            {:else if param.kind === 'enum'}
                <EnumDropdown
                    value={String((param.value ?? param.default) as number)}
                    options={(param.options ?? []).map((label, i) => [String(i), label])}
                    disabled={param.name === 'model' && colorizeOn}
                    onchange={(k) => onEnumChange(param, k)}
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

    {#if (params ?? []).length === 0}
        <div class="empty">No parameters</div>
    {/if}
</div>

<style>
    .filter-params {
        display: flex;
        flex-direction: column;
        gap: 8px;
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
