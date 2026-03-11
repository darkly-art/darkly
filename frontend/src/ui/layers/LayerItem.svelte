<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getLayerThumbnail, getMaskThumbnail, THUMB_SIZE } from './thumbnails';
    import { dispatchBinding } from '../../actions/triggers';
    import { actions } from '../../actions/registry';
    import { config } from '../../config/store.svelte';

    let { layer, depth = 0, onupdate }: {
        layer: {
            type: string; id: number; name: string; visible: boolean;
            opacity?: number; blendMode?: number;
            hasMask?: boolean; maskEnabled?: boolean; showMask?: boolean;
        };
        depth?: number;
        onupdate: () => void;
    } = $props();

    let isActive = $derived(app.activeLayerId === layer.id);
    let isEditingMask = $derived(app.editingMaskLayerId === layer.id);
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below'>('none');

    // Thumbnails — regenerated when layer data changes (onupdate triggers re-render)
    let layerThumb = $derived(layer.type === 'raster' && app.handle ? getLayerThumbnail(layer.id) : '');
    let maskThumb = $derived(layer.hasMask && app.handle ? getMaskThumbnail(layer.id) : '');

    // Mask context menu
    let showMaskMenu = $state(false);
    let maskMenuX = $state(0);
    let maskMenuY = $state(0);

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (dispatchBinding('layerEye', e, { layerId: layer.id }, config)) {
            onupdate();
            return;
        }
        // Default: plain click toggles visibility
        actions.dispatch('toggleVisibility', { layerId: layer.id });
        onupdate();
    }

    function setActive() {
        app.activeLayerId = layer.id;
        // If we're clicking the layer row (not the mask thumb), switch to layer editing
        if (isEditingMask) {
            app.editingMaskLayerId = null;
            app.handle?.set_editing_mask(layer.id, false);
            onupdate();
        }
    }

    function clickLayerThumb(e: MouseEvent) {
        e.stopPropagation();
        app.activeLayerId = layer.id;
        if (isEditingMask) {
            app.editingMaskLayerId = null;
            app.handle?.set_editing_mask(layer.id, false);
            onupdate();
        }
    }

    function clickMaskThumb(e: MouseEvent) {
        e.stopPropagation();
        if (dispatchBinding('maskThumb', e, { layerId: layer.id }, config)) {
            onupdate();
            return;
        }
        // Default: plain click toggles mask editing mode
        app.activeLayerId = layer.id;
        if (!isEditingMask) {
            app.editingMaskLayerId = layer.id;
            app.handle?.set_editing_mask(layer.id, true);
        } else {
            app.editingMaskLayerId = null;
            app.handle?.set_editing_mask(layer.id, false);
        }
        onupdate();
    }

    function onMaskContextMenu(e: MouseEvent) {
        e.preventDefault();
        e.stopPropagation();
        maskMenuX = e.clientX;
        maskMenuY = e.clientY;
        showMaskMenu = true;

        // Close on next click anywhere
        const close = () => { showMaskMenu = false; document.removeEventListener('click', close); };
        requestAnimationFrame(() => document.addEventListener('click', close));
    }

    function toggleMaskEnabled() {
        if (app.handle) {
            app.handle.set_mask_enabled(layer.id, !layer.maskEnabled);
            onupdate();
        }
    }

    function toggleShowMask() {
        if (app.handle) {
            app.handle.set_show_mask(layer.id, !layer.showMask);
            onupdate();
        }
    }

    function applyMask() {
        if (app.handle) {
            if (isEditingMask) {
                app.editingMaskLayerId = null;
                app.handle.set_editing_mask(layer.id, false);
            }
            app.handle.apply_mask(layer.id);
            onupdate();
        }
    }

    function removeMask() {
        if (app.handle) {
            if (isEditingMask) {
                app.editingMaskLayerId = null;
                app.handle.set_editing_mask(layer.id, false);
            }
            app.handle.remove_mask(layer.id);
            onupdate();
        }
    }

    function startRename() {
        if (layer.type !== 'raster') return;
        editing = true;
        requestAnimationFrame(() => editInput?.focus());
    }

    function finishRename() {
        editing = false;
        if (app.handle && editInput) {
            app.handle.set_layer_name(layer.id, editInput.value);
            onupdate();
        }
    }

    function onOpacityChange(e: Event) {
        const value = parseFloat((e.target as HTMLInputElement).value);
        if (app.handle) {
            app.handle.set_opacity(layer.id, value);
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
            app.handle.move_layer(id, 'after', layer.id);
        } else {
            app.handle.move_layer(id, 'before', layer.id);
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

    {#if layer.type === 'raster' && layerThumb}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <img
            class="thumb"
            class:thumb-active={isActive && !isEditingMask}
            src={layerThumb}
            alt="layer"
            width={THUMB_SIZE}
            height={THUMB_SIZE}
            onclick={clickLayerThumb}
        />
    {/if}

    {#if layer.hasMask && maskThumb}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <img
            class="thumb"
            class:thumb-active={isEditingMask}
            class:mask-disabled={!layer.maskEnabled}
            src={maskThumb}
            alt="mask"
            width={THUMB_SIZE}
            height={THUMB_SIZE}
            onclick={clickMaskThumb}
            oncontextmenu={onMaskContextMenu}
        />
    {/if}

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

{#if showMaskMenu}
    <div class="mask-menu" style:left="{maskMenuX}px" style:top="{maskMenuY}px">
        <button onclick={toggleMaskEnabled}>
            {layer.maskEnabled ? 'Disable mask' : 'Enable mask'}
        </button>
        <button onclick={toggleShowMask}>
            {layer.showMask ? 'Hide mask' : 'Show mask'}
        </button>
        <button onclick={applyMask}>Apply mask</button>
        <button onclick={removeMask}>Remove mask</button>
    </div>
{/if}

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
        flex-shrink: 0;
    }
    .vis-btn.hidden { color: #444; }

    .thumb {
        border: 2px solid #444;
        border-radius: 2px;
        flex-shrink: 0;
        cursor: pointer;
        image-rendering: pixelated;
    }

    .thumb-active {
        border-color: #6a6aff;
    }

    .mask-disabled {
        opacity: 0.4;
    }

    .layer-name {
        flex: 1;
        font-size: 12px;
        color: #ccc;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        min-width: 0;
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
        min-width: 0;
    }

    .opacity-slider {
        width: 50px;
        height: 12px;
        accent-color: #6a6aff;
        flex-shrink: 0;
    }

    .mask-menu {
        position: fixed;
        z-index: 1000;
        background: #2a2a2a;
        border: 1px solid #555;
        border-radius: 4px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
    }

    .mask-menu button {
        display: block;
        width: 100%;
        background: none;
        border: none;
        color: #ccc;
        font-size: 12px;
        padding: 6px 16px;
        text-align: left;
        cursor: pointer;
        white-space: nowrap;
    }

    .mask-menu button:hover {
        background: #3a3a4a;
    }
</style>
