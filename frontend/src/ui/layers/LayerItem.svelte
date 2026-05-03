<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail, THUMB_SIZE } from './thumbnails';
    import { dispatchClick } from '../../actions/triggers';
    import { actions } from '../../actions/registry';

    interface Modifier {
        id: number; kind: string; name: string; visible: boolean; locked: boolean;
    }

    let { layer, depth = 0, onupdate }: {
        layer: {
            type: string; id: number; name: string; visible: boolean; locked?: boolean;
            opacity?: number; blendMode?: number;
            modifiers?: Modifier[];
        };
        depth?: number;
        onupdate: () => void;
    } = $props();

    // The mask modifier (if any) is one of the host's modifiers. The model
    // permits N; the UI exposes one.
    let maskModifier = $derived<Modifier | null>(
        layer.modifiers?.find((m) => m.kind === 'mask') ?? null,
    );
    let hasMask = $derived(maskModifier !== null);
    let maskEnabled = $derived(maskModifier?.visible ?? true);
    let isMaskIsolated = $derived(
        maskModifier !== null && app.isolatedNodeId === maskModifier.id,
    );

    let isActive = $derived(app.activeLayerId === layer.id);
    // The mask is the active edit target whenever the active node id IS the
    // mask modifier id — no session redirect.
    let isEditingMask = $derived(
        maskModifier !== null && app.activeLayerId === maskModifier.id,
    );
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below'>('none');

    let layerThumb = $derived(layer.type === 'raster' && app.handle ? getNodeThumbnail(layer.id) : '');
    let maskThumb = $derived(maskModifier !== null && app.handle ? getNodeThumbnail(maskModifier.id) : '');

    let showMaskMenu = $state(false);
    let maskMenuX = $state(0);
    let maskMenuY = $state(0);

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (dispatchClick('layerEye', e, { layerId: layer.id })) {
            onupdate();
            return;
        }
        actions.dispatch('toggleVisibility', { layerId: layer.id });
        onupdate();
    }

    function setActive() {
        app.selectLayer(layer.id);
    }

    function clickLayerThumb(e: MouseEvent) {
        e.stopPropagation();
        app.selectLayer(layer.id);
    }

    function clickMaskThumb(e: MouseEvent) {
        e.stopPropagation();
        if (dispatchClick('maskThumb', e, { layerId: layer.id })) {
            onupdate();
            return;
        }
        // Default: plain click activates the mask modifier as the paint
        // target — the active node id IS the modifier id.
        if (maskModifier === null) return;
        app.selectLayer(maskModifier.id);
    }

    function onMaskContextMenu(e: MouseEvent) {
        e.preventDefault();
        e.stopPropagation();
        maskMenuX = e.clientX;
        maskMenuY = e.clientY;
        showMaskMenu = true;

        const close = () => { showMaskMenu = false; document.removeEventListener('click', close); };
        requestAnimationFrame(() => document.addEventListener('click', close));
    }

    function toggleMaskEnabled() {
        if (app.handle && maskModifier !== null) {
            app.handle.set_layer_visible(maskModifier.id, !maskEnabled);
            onupdate();
        }
    }

    function toggleShowMask() {
        if (app.handle && maskModifier !== null) {
            const next = isMaskIsolated ? 0 : maskModifier.id;
            app.handle.set_isolated_node(next);
            app.isolatedNodeId = next === 0 ? null : next;
            onupdate();
        }
    }

    function applyMask() {
        if (app.handle) {
            app.handle.apply_mask(layer.id);
            onupdate();
        }
    }

    function removeMask() {
        if (app.handle) {
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

    let draggable = $state(true);

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('text/plain', String(layer.id));
        if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
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
    draggable={draggable ? 'true' : 'false'}
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
        onpointerdown={(e: PointerEvent) => { e.stopPropagation(); draggable = false; }}
        onpointerup={() => { draggable = true; }}
        onpointerleave={() => { draggable = true; }}
        title="Toggle visibility"
    >
        <i class={layer.visible ? 'fa-solid fa-eye' : 'fa-solid fa-eye-slash'}></i>
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
            draggable="false"
            onclick={clickLayerThumb}
        />
    {/if}

    {#if hasMask && maskThumb}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <img
            class="thumb"
            class:thumb-active={isEditingMask}
            class:mask-disabled={!maskEnabled}
            src={maskThumb}
            alt="mask"
            width={THUMB_SIZE}
            height={THUMB_SIZE}
            draggable="false"
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
</div>

{#if showMaskMenu}
    <div class="mask-menu" style:left="{maskMenuX}px" style:top="{maskMenuY}px">
        <button onclick={toggleMaskEnabled}>
            {maskEnabled ? 'Disable mask' : 'Enable mask'}
        </button>
        <button onclick={toggleShowMask}>
            {isMaskIsolated ? 'Hide mask' : 'Show mask'}
        </button>
        <button onclick={applyMask}>Apply mask</button>
        <button onclick={removeMask}>Remove mask</button>
    </div>
{/if}

<style>
    .layer-item {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 12px;
        cursor: pointer;
        min-height: 28px;
        position: relative;
        transition: background 0.1s;
    }

    .layer-item:hover {
        background: var(--bg-hover);
    }

    .layer-item.active {
        background: var(--bg-active);
    }

    .layer-item.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .layer-item.drop-below::after {
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
        width: 24px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
        flex-shrink: 0;
        border-radius: 4px;
        transition: color 0.1s;
    }
    .vis-btn:hover { color: var(--text); }
    .vis-btn.hidden { color: var(--text-dim); }

    .thumb {
        width: 32px;
        height: 32px;
        border: 2px solid var(--text-dim);
        border-radius: 4px;
        flex-shrink: 0;
        cursor: pointer;
        image-rendering: pixelated;
        background: var(--thumb-bg);
    }

    .thumb-active {
        border-color: var(--accent);
    }

    .mask-disabled {
        opacity: 0.4;
    }

    .layer-name {
        flex: 1;
        font-size: 12px;
        color: var(--text);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        min-width: 0;
    }

    .name-input {
        flex: 1;
        background: var(--bg);
        border: 1px solid var(--accent);
        border-radius: 2px;
        color: var(--text);
        font-size: 12px;
        padding: 1px 4px;
        outline: none;
        min-width: 0;
    }

    .mask-menu {
        position: fixed;
        z-index: 1000;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
    }

    .mask-menu button {
        display: block;
        width: 100%;
        background: none;
        border: none;
        color: var(--text);
        font-size: 12px;
        padding: 6px 16px;
        text-align: left;
        cursor: pointer;
        white-space: nowrap;
    }

    .mask-menu button:hover {
        background: var(--bg-hover);
    }
</style>
