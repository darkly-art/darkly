<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail, THUMB_SIZE } from './thumbnails';
    import { bindingSite } from '../../actions/binding_site';
    import { actions } from '../../actions/registry';

    interface Modifier {
        id: number; kind: string; name: string; visible: boolean; locked: boolean;
    }

    let { layer, depth = 0, onupdate }: {
        layer: {
            type: string; id: number; name: string; visible: boolean; locked?: boolean;
            opacity?: number; blendMode?: string;
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

    let showLayerMenu = $state(false);
    let layerMenuX = $state(0);
    let layerMenuY = $state(0);

    /// Walk the layer tree to determine whether `id`'s parent has a child
    /// directly below it. `app.layerTree` is top-to-bottom (top of stack at
    /// index 0), so "sibling below" = sibling at a higher index.
    function siblingBelowExists(nodes: any[], id: number): boolean {
        for (const n of nodes) {
            if (n.id === id) return false; // root-level, handled by caller
            if (n.children) {
                const idx = n.children.findIndex((c: any) => c.id === id);
                if (idx >= 0) return idx < n.children.length - 1;
                if (siblingBelowExists(n.children, id)) return true;
            }
        }
        return false;
    }

    let canMergeDownForThis = $derived.by(() => {
        const topIdx = app.layerTree.findIndex((n: any) => n.id === layer.id);
        if (topIdx >= 0) return topIdx < app.layerTree.length - 1;
        return siblingBelowExists(app.layerTree, layer.id);
    });

    // Chord dispatch is owned by `use:bindingSite` on each preview
    // element below — `bindingSite` intercepts modifier+click in capture
    // phase and dispatches against its named site. These onclick handlers
    // are the no-chord fallback (plain click → select / toggle visibility).
    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleVisibility', { layerId: layer.id });
        onupdate();
    }

    function onLayerClick() {
        // The layer-item body has no chord bindings — modifier+click is
        // reserved for the previews. Plain click selects.
        app.selectLayer(layer.id);
    }

    function clickLayerThumb(e: MouseEvent) {
        e.stopPropagation();
        app.selectLayer(layer.id);
    }

    function clickMaskThumb(e: MouseEvent) {
        e.stopPropagation();
        if (maskModifier === null) return;
        // Activating the mask = setting the active node id to the modifier's
        // id. There is no separate "edit mask" redirect.
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

    function onLayerContextMenu(e: MouseEvent) {
        e.preventDefault();
        e.stopPropagation();
        // Select the right-clicked layer so subsequent dispatched actions
        // that fall back to `app.activeLayerId` (e.g. when ctx.layerId
        // isn't honoured by a particular handler) still target this layer.
        app.selectLayer(layer.id);
        layerMenuX = e.clientX;
        layerMenuY = e.clientY;
        showLayerMenu = true;

        const close = () => { showLayerMenu = false; document.removeEventListener('click', close); };
        requestAnimationFrame(() => document.addEventListener('click', close));
    }

    function menuDuplicate() {
        actions.dispatch('duplicateLayer', { layerId: layer.id });
        onupdate();
    }

    function menuMergeDown() {
        if (!canMergeDownForThis) return;
        actions.dispatch('mergeDown', { layerId: layer.id });
        onupdate();
    }

    function menuFlatten() {
        if (!hasMask) return;
        actions.dispatch('flatten', { layerId: layer.id });
        onupdate();
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
    onclick={onLayerClick}
    ondblclick={startRename}
    oncontextmenu={onLayerContextMenu}
    role="button"
    tabindex="-1"
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
        use:bindingSite={{ name: 'layerEye', ctx: () => ({ layerId: layer.id }) }}
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
            use:bindingSite={{ name: 'layerThumb', ctx: () => ({ layerId: layer.id }) }}
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
            use:bindingSite={{ name: 'maskThumb', ctx: () => ({ layerId: maskModifier!.id }) }}
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
        <button onclick={removeMask}>Delete mask</button>
    </div>
{/if}

{#if showLayerMenu}
    <div class="layer-menu" style:left="{layerMenuX}px" style:top="{layerMenuY}px">
        <button onclick={menuDuplicate}>
            Duplicate layer
        </button>
        <button onclick={menuMergeDown} disabled={!canMergeDownForThis}>
            Merge down
        </button>
        {#if hasMask}
            <button onclick={menuFlatten}>Flatten</button>
        {/if}
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
        user-select: none;
    }

    .layer-item:focus,
    .layer-item:focus-visible {
        outline: none;
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

    .layer-menu {
        position: fixed;
        z-index: 1000;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
        min-width: 160px;
    }

    .layer-menu button {
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

    .layer-menu button:hover:not(:disabled) {
        background: var(--bg-hover);
    }

    .layer-menu button:disabled {
        color: var(--text-dim);
        cursor: default;
    }

    .layer-menu-sep {
        height: 1px;
        background: var(--bg-hover);
        margin: 4px 0;
    }
</style>
