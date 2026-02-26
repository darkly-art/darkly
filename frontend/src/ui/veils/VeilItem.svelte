<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { veil, onupdate }: {
        veil: { type: string; visible: boolean; index: number };
        onupdate: () => void;
    } = $props();

    let dropPos = $state<'none' | 'above' | 'below'>('none');

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.set_veil_visible(veil.index, !veil.visible);
            onupdate();
        }
    }

    function remove(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.remove_veil(veil.index);
            onupdate();
        }
    }

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('application/x-veil', String(veil.index));
        if (e.dataTransfer) {
            e.dataTransfer.effectAllowed = 'move';
        }
    }

    function onDragOver(e: DragEvent) {
        if (!e.dataTransfer?.types.includes('application/x-veil')) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = 'move';

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        dropPos = ratio < 0.5 ? 'above' : 'below';
    }

    function onDragLeave(e: DragEvent) {
        const related = e.relatedTarget as Node | null;
        if (!related || !(e.currentTarget as HTMLElement).contains(related)) {
            dropPos = 'none';
        }
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        dropPos = 'none';
        const draggedIdx = e.dataTransfer?.getData('application/x-veil');
        if (draggedIdx == null || !app.handle) return;
        const from = Number(draggedIdx);
        if (from === veil.index) return;

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        let to = ratio < 0.5 ? veil.index : veil.index + 1;
        // Adjust for removal shifting indices
        if (from < to) to--;

        app.handle.move_veil(from, to);
        onupdate();
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="veil-item"
    class:drop-above={dropPos === 'above'}
    class:drop-below={dropPos === 'below'}
    draggable="true"
    ondragstart={onDragStart}
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
    ondragend={() => { dropPos = 'none'; }}
>
    <button
        class="vis-btn"
        class:hidden={!veil.visible}
        onclick={toggleVisibility}
        title="Toggle visibility"
    >
        {veil.visible ? '\u{1F441}' : '\u{2014}'}
    </button>

    <span class="veil-name">{veil.type}</span>

    <button
        class="remove-btn"
        onclick={remove}
        title="Remove veil"
    >
        &times;
    </button>
</div>

<style>
    .veil-item {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 4px 8px;
        min-height: 24px;
        position: relative;
        cursor: grab;
    }

    .veil-item:hover {
        background: #2a2a2a;
    }

    .veil-item.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: #6a6aff;
        pointer-events: none;
    }

    .veil-item.drop-below::after {
        content: '';
        position: absolute;
        bottom: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: #6a6aff;
        pointer-events: none;
    }

    .vis-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        padding: 0;
        font-size: 12px;
        width: 18px;
        text-align: center;
    }
    .vis-btn.hidden { color: #444; }

    .veil-name {
        flex: 1;
        font-size: 12px;
        color: #ccc;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .remove-btn {
        background: none;
        border: none;
        color: #555;
        cursor: pointer;
        padding: 0;
        font-size: 14px;
        width: 18px;
        text-align: center;
        line-height: 1;
    }
    .remove-btn:hover { color: #e44; }
</style>
