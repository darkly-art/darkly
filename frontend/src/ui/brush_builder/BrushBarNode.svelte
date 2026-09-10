<script lang="ts">
    import { getContext } from 'svelte';
    import { flip } from 'svelte/animate';
    import { cubicOut } from 'svelte/easing';
    import { brushGraph, type ExposedPortInfo } from '../../state/brush_graph.svelte';
    import Icon from '../../icons/Icon.svelte';
    import BrushBarEntryModal from './BrushBarEntryModal.svelte';
    import type { NodeCanvasContext } from './NodeCanvas.svelte';

    // Position is graph-space: the brush bar lives on the canvas
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

    // Drag-reorder state. `liveOrder` shadows the engine's port list
    // during a drag so the rows can shuffle in real time on every
    // `dragover`; `animate:flip` rides those reactive reorders and
    // slides the rows past the cursor. On drop we commit the final
    // index to the engine in one call; on cancel/escape we just drop
    // the shadow and revert to whatever the engine still says.
    let dragKey = $state<string | null>(null);
    let liveOrder = $state<ExposedPortInfo[] | null>(null);

    /** Rows the brush bar actually renders: the live drag shadow when
     *  a drag is in flight, otherwise the engine's order. */
    let displayPorts = $derived(liveOrder ?? brushGraph.exposedPorts);

    function onRowDragStart(e: DragEvent, port: ExposedPortInfo) {
        dragKey = port.key;
        // Snapshot the current order as a mutable shadow we'll
        // splice the dragged entry through on every dragover.
        liveOrder = [...brushGraph.exposedPorts];
        if (e.dataTransfer) {
            e.dataTransfer.effectAllowed = 'move';
            e.dataTransfer.setData('text/plain', port.key);
        }
    }

    function onRowDragOver(e: DragEvent, hoverIdx: number) {
        if (dragKey === null || !liveOrder) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const insertBelow = (e.clientY - rect.top) / rect.height >= 0.5;

        const fromIdx = liveOrder.findIndex((p) => p.key === dragKey);
        if (fromIdx === -1) return;

        let target = hoverIdx + (insertBelow ? 1 : 0);
        if (fromIdx < target) target -= 1;
        target = Math.max(0, Math.min(liveOrder.length - 1, target));
        if (target === fromIdx) return;

        // Reassign with a fresh array so Svelte's keyed-each diff
        // notices the reorder and animate:flip can play.
        const next = liveOrder.slice();
        const [moved] = next.splice(fromIdx, 1);
        next.splice(target, 0, moved);
        liveOrder = next;
    }

    function onRowDragLeave() {
        // No-op: leaving one row to enter another is fine; the next
        // dragover sets the new target. Leaving the whole list is
        // handled by dragend.
    }

    function onRowDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        commitDrag();
    }

    function onRowDragEnd() {
        // Drop fired? We've already committed. Drop didn't fire (drag
        // ended outside any row or via Escape)? Discard the shadow.
        commitDrag();
    }

    function commitDrag() {
        const key = dragKey;
        const order = liveOrder;
        if (key !== null && order) {
            const finalIdx = order.findIndex((p) => p.key === key);
            const engineIdx = brushGraph.exposedPorts.findIndex((p) => p.key === key);
            if (finalIdx !== -1 && finalIdx !== engineIdx) {
                // Sync engine state synchronously; the resulting engine
                // order will match liveOrder, so clearing the shadow
                // below doesn't flash the row back to its old slot.
                brushGraph.reorderExposedPort(key, finalIdx);
            }
        }
        dragKey = null;
        liveOrder = null;
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
        <Icon name="fa6-solid:sliders" class="header-icon" />
        <span class="title">Brush Bar</span>
    </header>
    <div class="body">
        {#if brushGraph.exposedPorts.length === 0}
            <p class="empty">
                Click the <Icon name="fa6-solid:eye" /> icon on any port to add it here.
            </p>
        {:else}
            <ul class="rows" ondragend={onRowDragEnd}>
                {#each displayPorts as port, idx (port.key)}
                    <li
                        class="entry-row"
                        class:dragging={dragKey === port.key}
                        animate:flip={{ duration: 100, easing: cubicOut }}
                        draggable="true"
                        ondragstart={(e) => onRowDragStart(e, port)}
                        ondragover={(e) => onRowDragOver(e, idx)}
                        ondragleave={onRowDragLeave}
                        ondrop={onRowDrop}
                    >
                        <span class="row-grip" aria-hidden="true">
                            <Icon name="fa6-solid:grip-vertical" />
                        </span>
                        {#if port.icon}
                            <span class="row-icon"><Icon name={port.icon} /></span>
                        {/if}
                        <span class="row-label">{port.label}</span>
                        <button
                            class="row-edit"
                            title="Edit label, description, and icon"
                            onclick={(e) => { e.stopPropagation(); openEditor(port); }}
                            ondragstart={(e) => e.preventDefault()}
                        >
                            <Icon name="fa6-solid:pen" />
                        </button>
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
    .header :global(.header-icon) {
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
    .entry-row.dragging {
        opacity: 0.55;
        border-color: var(--accent);
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
    .row-edit {
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
        padding: 2px 4px;
        border-radius: 3px;
        opacity: 0;
        transition: opacity 0.1s, color 0.1s;
    }
    .entry-row:hover .row-edit {
        opacity: 0.8;
    }
    .row-edit:hover {
        color: var(--text);
        background: color-mix(in srgb, var(--accent) 18%, transparent);
        opacity: 1;
    }
</style>
