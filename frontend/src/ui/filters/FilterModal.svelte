<script lang="ts">
    import Modal from '../Modal.svelte';
    import FilterParamsEditor from './FilterParamsEditor.svelte';
    import { app } from '../../state/app.svelte';
    import { filterModal } from '../../state/filterModal.svelte';
    import { seedScratchParams, filterParamMap, type FilterParam } from './filterParams';

    // Scratch params edited in the dialog — seeded from the schema defaults each
    // time it opens, never touching the shared schema array.
    let scratch = $state<FilterParam[]>([]);

    const HIST_BINS = 256;
    const showsLevels = $derived(scratch.some((p) => p.kind === 'levels'));
    let histogramBins = $state<Uint32Array | null>(null);

    let prevOpen = false;
    $effect(() => {
        if (filterModal.open && !prevOpen) {
            scratch = seedScratchParams(filterModal.schema);
        }
        prevOpen = filterModal.open;
    });

    // For a destructive Levels edit there's no filter layer in the tree to bin,
    // so histogram the target node's *own* texture (see `request_node_histogram`).
    $effect(() => {
        const engine = app.engine;
        const id = filterModal.nodeId;
        if (!engine || !filterModal.open || !showsLevels || id === null) return;
        engine.post('request_node_histogram', { id });
        let stopped = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        const poll = () => {
            if (stopped) return;
            engine
                .send<{ bytes: Uint8Array }>('histogram_result', { id })
                .then(({ bytes }) => {
                    if (stopped) return;
                    if (bytes && bytes.length >= 8 * HIST_BINS * 4) {
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
            engine.post('request_node_histogram', { id: -1 });
        };
    });

    function close() {
        filterModal.open = false;
    }

    async function apply() {
        const engine = app.engine;
        const id = filterModal.nodeId;
        if (engine && id !== null) {
            await engine.api.applyFilter({
                node_id: id,
                filter_type: filterModal.filterType,
                params: filterParamMap(scratch),
            });
            app.requestFrame();
        }
        close();
    }
</script>

<Modal bind:open={filterModal.open} title={filterModal.displayName} size="sm">
    <div class="body">
        <FilterParamsEditor params={scratch} {histogramBins} />
        <div class="actions">
            <button type="button" class="cancel" onclick={close}>Cancel</button>
            <button type="button" class="ok" onclick={apply}>Apply</button>
        </div>
    </div>
</Modal>

<style>
    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 300px;
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 10px;
    }

    .cancel,
    .ok {
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 7px 16px;
        cursor: pointer;
        font-size: 13px;
    }

    .cancel {
        background: transparent;
        color: var(--text-muted);
    }

    .ok {
        background: var(--accent, var(--bg-hover));
        color: var(--text);
        border-color: transparent;
    }
</style>
