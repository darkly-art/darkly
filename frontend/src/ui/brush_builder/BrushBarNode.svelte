<script lang="ts">
    import { getContext } from 'svelte';
    import { brushGraph, type ExposedPortInfo } from '../../state/brush_graph.svelte';
    import BrushBarEntryModal from './BrushBarEntryModal.svelte';
    import type { NodeCanvasContext } from './NodeCanvas.svelte';

    // Position is graph-space — the brush bar lives on the canvas
    // alongside the other nodes, panning and zooming with them. Default
    // sits a comfortable margin above and to the left of the origin so
    // the auto-laid-out brush graph doesn't overlap it on first open.
    // Component-local: not persisted across reloads.
    let pos = $state({ x: -360, y: -40 });
    const { coords } = getContext<NodeCanvasContext>('node-canvas');

    // Drag-move (whole-node) state.
    let movingNode = false;
    let moveStart = { px: 0, py: 0, x: 0, y: 0 };

    function startMove(e: PointerEvent) {
        // Ignore drag-init when the gesture started on an interactive
        // child (row drag handle, edit button, etc.).
        const target = e.target as HTMLElement;
        if (target.closest('.entry-row, button, input, textarea')) return;
        movingNode = true;
        moveStart = { px: e.clientX, py: e.clientY, x: pos.x, y: pos.y };
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        e.preventDefault();
        e.stopPropagation();
    }
    function moveNode(e: PointerEvent) {
        if (!movingNode) return;
        // Convert client-px delta to graph-units so the node moves with
        // the cursor at any zoom level.
        const d = coords.clientDeltaToGraph(
            e.clientX - moveStart.px,
            e.clientY - moveStart.py,
        );
        pos = { x: moveStart.x + d.x, y: moveStart.y + d.y };
    }
    function endMove(e: PointerEvent) {
        if (!movingNode) return;
        movingNode = false;
        (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    }

    // Drag-reorder state for rows.
    let dragKey = $state<string | null>(null);
    let dropIndicator = $state<{ index: number; pos: 'above' | 'below' } | null>(null);

    function onRowDragStart(e: DragEvent, port: ExposedPortInfo) {
        dragKey = port.key;
        if (e.dataTransfer) {
            e.dataTransfer.effectAllowed = 'move';
            e.dataTransfer.setData('text/plain', port.key);
        }
    }
    function onRowDragOver(e: DragEvent, idx: number) {
        if (dragKey === null) return;
        e.preventDefault();
        if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        dropIndicator = { index: idx, pos: ratio < 0.5 ? 'above' : 'below' };
    }
    function onRowDragLeave() {
        // Don't reset on leave — the next dragover wins.
    }
    function onRowDrop(e: DragEvent) {
        e.preventDefault();
        if (dragKey === null || !dropIndicator) {
            dragKey = null;
            dropIndicator = null;
            return;
        }
        // Compute the target index: insertion point above/below the
        // hovered row. Then collapse to a stable index for the engine
        // call (which moves the entry to that position).
        let target = dropIndicator.index;
        if (dropIndicator.pos === 'below') target += 1;
        // Account for the dragged entry's own removal from earlier in
        // the list — when we move forward past our origin, the target
        // shifts back by one.
        const fromIdx = brushGraph.exposedPorts.findIndex((p) => p.key === dragKey);
        if (fromIdx >= 0 && fromIdx < target) target -= 1;
        if (target < 0) target = 0;
        const last = brushGraph.exposedPorts.length - 1;
        if (target > last) target = last;
        if (target !== fromIdx) {
            brushGraph.reorderExposedPort(dragKey, target);
        }
        dragKey = null;
        dropIndicator = null;
    }
    function onRowDragEnd() {
        dragKey = null;
        dropIndicator = null;
    }

    // Edit modal state.
    let modalOpen = $state(false);
    let modalEntry = $state<ExposedPortInfo | null>(null);
    function openEditor(port: ExposedPortInfo) {
        modalEntry = port;
        modalOpen = true;
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="brush-bar-node"
    style="transform: translate({pos.x}px, {pos.y}px);"
    onpointerdown={startMove}
    onpointermove={moveNode}
    onpointerup={endMove}
    onlostpointercapture={endMove}
>
    <header class="header">
        <i class="fa-solid fa-sliders header-icon"></i>
        <span class="title">Brush Bar</span>
    </header>
    <div class="body">
        {#if brushGraph.exposedPorts.length === 0}
            <p class="empty">
                Click the <i class="fa-solid fa-eye"></i> icon on any port to add it here.
            </p>
        {:else}
            <ul class="rows" ondragend={onRowDragEnd}>
                {#each brushGraph.exposedPorts as port, idx (port.key)}
                    <!-- svelte-ignore a11y_click_events_have_key_events -->
                    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                    <li
                        class="entry-row"
                        class:drag-above={dropIndicator?.index === idx && dropIndicator?.pos === 'above'}
                        class:drag-below={dropIndicator?.index === idx && dropIndicator?.pos === 'below'}
                        draggable="true"
                        ondragstart={(e) => onRowDragStart(e, port)}
                        ondragover={(e) => onRowDragOver(e, idx)}
                        ondragleave={onRowDragLeave}
                        ondrop={onRowDrop}
                        onclick={() => openEditor(port)}
                    >
                        <span class="row-grip" aria-hidden="true">
                            <i class="fa-solid fa-grip-vertical"></i>
                        </span>
                        {#if port.icon}
                            <span class="row-icon"><i class={port.icon}></i></span>
                        {/if}
                        <span class="row-label">{port.label}</span>
                    </li>
                {/each}
            </ul>
        {/if}
    </div>
</div>

<BrushBarEntryModal bind:open={modalOpen} entry={modalEntry} />

<style>
    .brush-bar-node {
        position: absolute;
        top: 0;
        left: 0;
        z-index: 6;
        min-width: 180px;
        max-width: 260px;
        background: var(--bg-active);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
        font-size: 11px;
        user-select: none;
        cursor: move;
    }
    .header {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 8px 10px;
        border-bottom: 1px solid var(--bg-hover);
        font-size: 12px;
        font-weight: 600;
    }
    .header-icon {
        color: var(--accent);
    }
    .body {
        padding: 6px;
        cursor: default;
    }
    .empty {
        margin: 0;
        padding: 6px 4px;
        color: var(--text-muted);
        font-size: 11px;
        line-height: 1.4;
    }
    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 2px;
    }
    .entry-row {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 5px 6px;
        background: color-mix(in srgb, var(--bg) 80%, transparent);
        border: 1px solid transparent;
        border-radius: 4px;
        cursor: grab;
    }
    .entry-row:hover {
        background: color-mix(in srgb, var(--bg-hover) 60%, transparent);
        border-color: var(--bg-hover);
    }
    .entry-row.drag-above {
        border-top-color: var(--accent);
    }
    .entry-row.drag-below {
        border-bottom-color: var(--accent);
    }
    .row-grip {
        color: var(--text-muted);
        cursor: grab;
        font-size: 10px;
    }
    .row-icon {
        color: var(--text-muted);
        width: 14px;
        text-align: center;
    }
    .row-label {
        flex: 1;
        overflow: hidden;
        white-space: nowrap;
        text-overflow: ellipsis;
    }
</style>
