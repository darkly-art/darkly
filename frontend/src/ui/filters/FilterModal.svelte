<script lang="ts">
    import Icon from '../../icons/Icon.svelte';
    import FilterParamsEditor from './FilterParamsEditor.svelte';
    import { pointerDrag } from '../workspace/pointerDrag';
    import { app } from '../../state/app.svelte';
    import { filterModal } from '../../state/filterModal.svelte';
    import { seedScratchParams, filterParamMap, type FilterParam } from './filterParams';

    // A *non-modal*, draggable floating panel — deliberately not `Modal.svelte`,
    // which dims the canvas behind a backdrop. The whole point here is a live
    // preview: the destructive filter is applied non-destructively to the target
    // node as the user edits (`previewFilter`), so the canvas must stay visible.
    // OK commits it (one undo step); Cancel / Escape / close restores the pixels.

    // Scratch params edited in the panel — seeded from the schema defaults each
    // time it opens, never touching the shared schema array.
    let scratch = $state<FilterParam[]>([]);

    const HIST_BINS = 256;
    const showsLevels = $derived(scratch.some((p) => p.kind === 'levels'));
    let histogramBins = $state<Uint32Array | null>(null);

    // Floating position (top-left px). Centered on each open, then draggable.
    let pos = $state({ x: 0, y: 0 });
    let dragBase = { x: 0, y: 0 };
    let panelEl = $state<HTMLDivElement>();

    // Center the panel in the viewport. A first estimate (fixed width, guessed
    // height) avoids a corner flash; a rAF refine uses the real measured size
    // once the params render (Levels is much taller than HSV).
    function center() {
        pos = {
            x: Math.max(16, (window.innerWidth - 320) / 2),
            y: Math.max(16, window.innerHeight / 2 - 180),
        };
        requestAnimationFrame(() => {
            if (!panelEl) return;
            pos = {
                x: Math.max(16, (window.innerWidth - panelEl.offsetWidth) / 2),
                y: Math.max(16, (window.innerHeight - panelEl.offsetHeight) / 2),
            };
        });
    }
    // True once committed, so the close-cleanup doesn't also cancel (restore).
    let committed = false;

    let prevOpen = false;
    $effect(() => {
        if (filterModal.open && !prevOpen) {
            scratch = seedScratchParams(filterModal.schema);
            committed = false;
            center();
            // Establish the preview session up front so the first edit is snappy.
            pushPreview();
        } else if (!filterModal.open && prevOpen && !committed) {
            // Closed by any path other than Apply — discard the live preview.
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

    function cancel() {
        if (!committed) {
            app.engine?.api.cancelFilterPreview();
            app.requestFrame();
        }
        filterModal.open = false;
    }

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

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Escape') {
            e.preventDefault();
            cancel();
        } else if (e.key === 'Enter' && (e.target as HTMLElement)?.tagName !== 'SELECT') {
            e.preventDefault();
            apply();
        }
    }
</script>

{#if filterModal.open}
    <div
        bind:this={panelEl}
        class="panel"
        style="left: {pos.x}px; top: {pos.y}px;"
        role="dialog"
        aria-label={filterModal.displayName}
        tabindex="-1"
        onkeydown={onKeydown}
    >
        <div
            class="titlebar"
            use:pointerDrag={{
                onStart: () => (dragBase = { ...pos }),
                onMove: (dx, dy) => (pos = { x: dragBase.x + dx, y: dragBase.y + dy }),
            }}
        >
            <span class="title">{filterModal.displayName}</span>
            <button class="close" onclick={cancel} title="Cancel" aria-label="Cancel">
                <Icon name="fa6-solid:xmark" />
            </button>
        </div>

        <div class="body">
            <FilterParamsEditor
                params={scratch}
                {histogramBins}
                oninput={pushPreview}
                onchange={pushPreview}
            />
            <div class="actions">
                <button type="button" class="cancel" onclick={cancel}>Cancel</button>
                <button type="button" class="ok" onclick={apply}>Apply</button>
            </div>
        </div>
    </div>
{/if}

<style>
    .panel {
        position: fixed;
        z-index: 60;
        width: 320px;
        max-width: calc(100vw - 32px);
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-md);
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }

    .titlebar {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding: 6px 8px 6px 12px;
        background: var(--bg-hover);
        cursor: grab;
        user-select: none;
        touch-action: none;
    }
    .titlebar:active {
        cursor: grabbing;
    }

    .title {
        font-size: 12px;
        font-weight: 600;
        text-transform: capitalize;
        color: var(--text);
    }

    .close {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 22px;
        height: 22px;
        background: transparent;
        border: none;
        border-radius: var(--radius-sm);
        color: var(--text-muted);
        cursor: pointer;
    }
    .close:hover {
        background: var(--bg-active);
        color: var(--text);
    }

    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
        padding: 12px;
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
