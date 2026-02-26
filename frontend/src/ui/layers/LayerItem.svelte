<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { layer, depth = 0, onupdate }: {
        layer: { type: string; id: number; name: string; visible: boolean; opacity?: number; blendMode?: number };
        depth?: number;
        onupdate: () => void;
    } = $props();

    let isActive = $derived(app.activeLayerId === layer.id);
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below'>('none');

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.set_layer_visible(BigInt(layer.id), !layer.visible);
            onupdate();
        }
    }

    function setActive() {
        app.activeLayerId = layer.id;
    }

    function startRename() {
        if (layer.type !== 'raster') return;
        editing = true;
        requestAnimationFrame(() => editInput?.focus());
    }

    function finishRename() {
        editing = false;
        if (app.handle && editInput) {
            app.handle.set_layer_name(BigInt(layer.id), editInput.value);
            onupdate();
        }
    }

    function onOpacityChange(e: Event) {
        const value = parseFloat((e.target as HTMLInputElement).value);
        if (app.handle) {
            app.handle.set_opacity(BigInt(layer.id), value);
            onupdate();
        }
    }

    let draggable = $state(true);

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('text/plain', String(layer.id));
    }

    function onDragOver(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        if (!e.dataTransfer) return;
        e.dataTransfer.dropEffect = 'move';

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        dropPos = ratio < 0.5 ? 'above' : 'below';
    }

    function onDragLeave(e: DragEvent) {
        // Only clear if leaving the element entirely (not entering a child)
        const related = e.relatedTarget as Node | null;
        if (!related || !(e.currentTarget as HTMLElement).contains(related)) {
            dropPos = 'none';
        }
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        dropPos = 'none';
        const draggedId = e.dataTransfer?.getData('text/plain');
        if (!draggedId || !app.handle) return;
        const id = Number(draggedId);
        if (id === layer.id) return;

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;

        if (ratio < 0.5) {
            app.handle.move_layer(BigInt(id), 'after', BigInt(layer.id));
        } else {
            app.handle.move_layer(BigInt(id), 'before', BigInt(layer.id));
        }
        onupdate();
    }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
    class="layer-item"
    class:active={isActive}
    class:drop-above={dropPos === 'above'}
    class:drop-below={dropPos === 'below'}
    onclick={setActive}
    ondblclick={startRename}
    onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setActive(); }}}
    role="button"
    tabindex="0"
    {draggable}
    ondragstart={onDragStart}
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
    ondragend={() => { dropPos = 'none'; }}
    style:padding-left="{8 + depth * 16}px"
>
    <button
        class="vis-btn"
        class:hidden={!layer.visible}
        onclick={toggleVisibility}
        title="Toggle visibility"
    >
        {layer.visible ? '\u{1F441}' : '\u{2014}'}
    </button>

    {#if editing}
        <input
            class="name-input"
            bind:this={editInput}
            value={layer.name}
            onblur={finishRename}
            onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') finishRename(); }}
            onclick={(e: MouseEvent) => e.stopPropagation()}
        />
    {:else}
        <span class="layer-name">{layer.name}</span>
    {/if}

    {#if layer.type === 'raster' && layer.opacity !== undefined}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <input
            type="range"
            class="opacity-slider"
            min="0" max="1" step="0.01"
            value={layer.opacity}
            oninput={onOpacityChange}
            onclick={(e: MouseEvent) => e.stopPropagation()}
            onpointerdown={() => { draggable = false; }}
            onpointerup={() => { draggable = true; }}
            onpointerleave={() => { draggable = true; }}
            title="Opacity: {Math.round((layer.opacity ?? 1) * 100)}%"
        />
    {/if}
</div>

<style>
    .layer-item {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 4px 8px;
        cursor: pointer;
        border-left: 3px solid transparent;
        min-height: 28px;
        position: relative;
    }

    .layer-item:hover {
        background: #2a2a2a;
    }

    .layer-item.active {
        background: #2a2a3a;
        border-left-color: #6a6aff;
    }

    .layer-item.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: #6a6aff;
        pointer-events: none;
    }

    .layer-item.drop-below::after {
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

    .layer-name {
        flex: 1;
        font-size: 12px;
        color: #ccc;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .name-input {
        flex: 1;
        background: #1a1a2a;
        border: 1px solid #6a6aff;
        border-radius: 2px;
        color: #ccc;
        font-size: 12px;
        padding: 1px 4px;
        outline: none;
    }

    .opacity-slider {
        width: 50px;
        height: 12px;
        accent-color: #6a6aff;
    }
</style>
