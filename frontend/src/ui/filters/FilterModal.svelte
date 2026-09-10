<script lang="ts">
    import Modal from '../Modal.svelte';
    import FilterParamsEditor from './FilterParamsEditor.svelte';
    import { app } from '../../state/app.svelte';
    import { filterModal } from '../../state/filterModal.svelte';
    import { seedScratchParams, filterParamMap, type ParamInfo } from './filterParams';

    // The destructive-apply dialog. Reuses `Modal` (so Escape / × / backdrop /
    // Cancel all close through one tested path) in its non-dimming + draggable
    // mode: the canvas stays visible and the filter is applied to the target
    // node *non-destructively* as the artist edits (`previewFilter`). Apply commits
    // it (one undo step); any close path restores the pixels.

    // Scratch params edited in the dialog, seeded from the schema defaults each
    // time it opens, never touching the shared schema array.
    let scratch = $state<ParamInfo[]>([]);

    const HIST_BINS = 256;
    const showsLevels = $derived(scratch.some((p) => p.kind === 'levels'));
    let histogramBins = $state<Uint32Array | null>(null);

    // True once committed, so the close-cleanup doesn't also cancel (restore).
    let committed = false;

    let prevOpen = false;
    $effect(() => {
        if (filterModal.open && !prevOpen) {
            scratch = seedScratchParams(filterModal.schema);
            committed = false;
            // Establish the preview session up front so the first edit is snappy.
            pushPreview();
        } else if (!filterModal.open && prevOpen && !committed) {
            // Closed by any path other than Apply: discard the live preview.
            app.engine?.api.cancelFilterPreview();
            app.requestFrame();
        }
        prevOpen = filterModal.open;
    });

    // Live-preview the current scratch params on the target node.
    function pushPreview() {
        const engine = app.engine;
        const id = filterModal.nodeId;
        if (!engine || !filterModal.open || id === null) return;
        engine.api.previewFilter({
            node_id: id,
            filter_type: filterModal.filterType,
            params: filterParamMap(scratch),
        });
        app.requestFrame();
    }

    // For a destructive Levels edit there's no filter layer in the tree to bin,
    // so histogram the target node's *own* texture (see `request_node_histogram`).
    $effect(() => {
        const engine = app.engine;
        const id = filterModal.nodeId;
        if (!engine || !filterModal.open || !showsLevels || id === null) return;
        engine.api.requestNodeHistogram({ id });
        let stopped = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        const poll = () => {
            if (stopped) return;
            engine.api
                .histogramResult({ id })
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
            engine.api.requestNodeHistogram({ id: -1 });
        };
    });

    // Cancel / × / Escape / backdrop all just close; the `$effect` above discards
    // the preview on close. Apply commits first, then closes.
    async function apply() {
        const engine = app.engine;
        const id = filterModal.nodeId;
        if (engine && id !== null) {
            committed = true;
            await engine.api.commitFilterPreview({
                node_id: id,
                filter_type: filterModal.filterType,
                params: filterParamMap(scratch),
            });
            app.requestFrame();
        }
        filterModal.open = false;
    }
</script>

<Modal bind:open={filterModal.open} title={filterModal.displayName} size="sm" dimmed={false} draggable>
    <div class="body">
        <FilterParamsEditor params={scratch} {histogramBins} oninput={pushPreview} onchange={pushPreview} />
        <div class="actions">
            <button type="button" class="cancel" onclick={() => (filterModal.open = false)}>Cancel</button>
            <button type="button" class="ok" onclick={apply}>Apply</button>
        </div>
    </div>
</Modal>

<style>
    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 260px;
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
