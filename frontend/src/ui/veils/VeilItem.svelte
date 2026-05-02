<script lang="ts">
    import { app } from '../../state/app.svelte';

    interface VeilParam {
        kind: 'float' | 'int' | 'bool';
        name: string;
        min?: number;
        max?: number;
        default: number | boolean;
        value?: number | boolean;
    }

    let { veil, onupdate }: {
        veil: { type: string; visible: boolean; index: number; params: VeilParam[] };
        onupdate: () => void;
    } = $props();

    let isActive = $derived(app.activeVeilIndex === veil.index);

    let dropPos = $state<'none' | 'above' | 'below'>('none');
    let draggable = $state(true);

    const ACRONYMS: Record<string, string> = { vhs: 'VHS' };

    function displayName(typeId: string): string {
        return ACRONYMS[typeId] ?? typeId.replace(/_/g, ' ');
    }

    function setActive() {
        app.selectVeil(veil.index);
    }

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.set_veil_visible(veil.index, !veil.visible);
            onupdate();
        }
    }

    function remove(e: MouseEvent) {
        e.stopPropagation();
        app.removeVeil(veil.index);
        onupdate();
    }

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('application/x-veil', String(veil.index));
        if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
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
        // UI list is reversed from internal order (top-of-list = highest index),
        // so "above" in the UI = after in internal order, matching layer convention.
        let to = ratio < 0.5 ? veil.index + 1 : veil.index;
        if (from < to) to--;

        app.moveVeil(from, to);
        onupdate();
    }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="veil-item"
    class:active={isActive}
    class:drop-above={dropPos === 'above'}
    class:drop-below={dropPos === 'below'}
    onclick={setActive}
    draggable={draggable ? 'true' : 'false'}
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
        onpointerdown={(e: PointerEvent) => { e.stopPropagation(); draggable = false; }}
        onpointerup={() => { draggable = true; }}
        onpointerleave={() => { draggable = true; }}
        title="Toggle visibility"
    >
        <i class={veil.visible ? 'fa-solid fa-eye' : 'fa-solid fa-eye-slash'}></i>
    </button>

    <span class="veil-name">{displayName(veil.type)}</span>

    <button
        class="remove-btn"
        onclick={remove}
        title="Remove veil"
    >
        <i class="fa-solid fa-trash"></i>
    </button>
</div>

<style>
    .veil-item {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 12px;
        min-height: 26px;
        cursor: pointer;
        position: relative;
        transition: background var(--transition-fast);
    }

    .veil-item:hover {
        background: color-mix(in srgb, var(--accent) 14%, transparent);
    }

    .veil-item.active {
        background: var(--bg-active);
    }

    .veil-item.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .veil-item.drop-below::after {
        content: '';
        position: absolute;
        bottom: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .vis-btn {
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        padding: 0;
        font-size: 12px;
        width: 18px;
        text-align: center;
        flex-shrink: 0;
        transition: color var(--transition-fast);
    }
    .vis-btn:hover { color: var(--text); }
    .vis-btn.hidden { color: var(--text-dim); }

    .veil-name {
        flex: 1;
        font-size: 12px;
        color: var(--text);
        text-transform: capitalize;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .remove-btn {
        background: none;
        border: none;
        color: var(--text-dim);
        cursor: pointer;
        padding: 0;
        font-size: 11px;
        width: 18px;
        text-align: center;
        flex-shrink: 0;
        transition: color var(--transition-fast);
    }
    .remove-btn:hover { color: var(--danger); }
</style>
