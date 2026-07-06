<script lang="ts">
    import { app } from '../../state/app.svelte';
    import FilterParamsEditor from './FilterParamsEditor.svelte';
    import { filterParamMap, type FilterParam } from './filterParams';

    let { node }: {
        node: { id: number; pipeline: string; params: FilterParam[] };
    } = $props();

    const filterLabel = $derived(app.filterDisplayName(node.pipeline));

    // --- Input histogram (Levels only) ---------------------------------------
    const HIST_BINS = 256;
    const showsLevels = $derived((node.params ?? []).some((p) => p.kind === 'levels'));
    // Stable primitive id: a layer-tree refresh replaces the `node` object but
    // keeps the same id, so keying the effect on this (not `node.id`) stops it
    // re-running — and re-fetching the histogram — on every param edit.
    const filterId = $derived(node.id);

    let histogramBins = $state<Uint32Array | null>(null);

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

    // Post the whole `{name: value}` param map (live, undo-coalesced). `refresh`
    // re-pulls the layer tree (skipped mid curve-drag so the live edit isn't
    // churned; the editor holds its own drag state either way).
    function pushParams(refresh: boolean) {
        if (!app.engine) return;
        app.engine.api.updateFilterParams({ id: node.id, params: filterParamMap(node.params) });
        if (refresh) app.refreshLayerTree();
        app.requestFrame();
    }
</script>

<div class="filter-props">
    <div class="header">
        <span class="type-label">{filterLabel}</span>
    </div>

    <FilterParamsEditor
        params={node.params ?? []}
        {histogramBins}
        oninput={() => pushParams(false)}
        onchange={() => pushParams(true)}
    />
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
</style>
