<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail, THUMB_SIZE } from './thumbnails.svelte';
    import { bindingSite } from '../../actions/binding_site';
    import { actions } from '../../actions/registry';
    import { tooltipForAction } from '../../config/store.svelte';
    import { toast } from '../../state/toast.svelte';
    import Icon from '../../icons/Icon.svelte';
    import ContextMenu, { type ContextMenuItem } from '../ContextMenu.svelte';

    interface Modifier {
        id: number; kind: string; name: string; visible: boolean; locked: boolean;
    }

    let { layer, depth = 0, onupdate }: {
        layer: {
            type: string; id: number; name: string; visible: boolean; locked?: boolean;
            // Mirrors `Document::is_node_editable` — false when this node OR
            // any ancestor is locked. `locked` is the node's own flag (drives
            // the icon); `editable` is the effective form (drives interaction
            // gates: rename, drag, mask/layer menu mutations).
            editable?: boolean;
            // Per-kind capability flags from the layer's registration (see
            // LayerKindRegistration). The panel reads these instead of
            // branching on `type` — a new layer kind declares its own and the
            // UI follows with no edit here.
            canHaveMask?: boolean;
            canRename?: boolean;
            hasThumbnail?: boolean;
            opacity?: number; blendMode?: string;
            modifiers?: Modifier[];
            // Iconify icon rendered as the panel thumbnail when the kind has no
            // live thumbnail (void: per-subtype; filter/group: per-kind).
            icon?: string;
            // Kind display name ("Void Layer", …) for the thumbnail tooltip.
            kindName?: string;
        };
        depth?: number;
        onupdate: () => void;
    } = $props();

    let editable = $derived(layer.editable !== false);

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
    let isSelected = $derived(app.isSelected(layer.id));
    let selectionSize = $derived(app.selectedLayerIds.size);
    let isMulti = $derived(selectionSize > 1);
    let deleteLabel = $derived(isMulti ? `Delete ${selectionSize} Layers` : 'Delete Layer');
    let dupLabel = $derived(isMulti ? `Duplicate ${selectionSize} Layers` : 'Duplicate Layer');
    let mergeLabel = $derived(isMulti ? `Merge ${selectionSize} Layers` : 'Merge Down');
    // The mask is the active edit target whenever the active node id IS the
    // mask modifier id — no session redirect.
    let isEditingMask = $derived(
        maskModifier !== null && app.activeLayerId === maskModifier.id,
    );
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below'>('none');

    let layerThumb = $derived(layer.hasThumbnail && app.engine ? getNodeThumbnail(layer.id) : '');
    let maskThumb = $derived(maskModifier !== null && app.engine ? getNodeThumbnail(maskModifier.id) : '');

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

    let canAddMask = $derived(Boolean(layer.canHaveMask) && !hasMask && editable);

    // Chord dispatch is owned by `use:bindingSite` on each preview
    // element below — `bindingSite` intercepts modifier+click in capture
    // phase and dispatches against its named site. These onclick handlers
    // are the no-chord fallback (plain click → select / toggle visibility).
    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleVisibility', { layerId: layer.id });
        onupdate();
    }

    function toggleLock(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleLock', { layerId: layer.id });
        onupdate();
    }

    function onLayerClick(e: MouseEvent) {
        // The layer-item body has no chord bindings — modifier+click is
        // reserved for the previews. Plain / ctrl / shift dispatch is
        // shared with LayerGroup via app.handleLayerRowClick.
        app.handleLayerRowClick(layer.id, e);
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
    }

    function onLayerContextMenu(e: MouseEvent) {
        e.preventDefault();
        e.stopPropagation();
        // If the right-clicked row is already in the multi-selection,
        // keep the selection intact — the menu acts on the whole set.
        // If it's not in the selection, replace the selection with just
        // this row (Photoshop / GIMP behavior). This way the menu and
        // every action it dispatches operate on a selection that
        // **always includes the right-clicked row**.
        if (!app.isSelected(layer.id)) {
            app.selectLayer(layer.id);
        }
        layerMenuX = e.clientX;
        layerMenuY = e.clientY;
        showLayerMenu = true;
    }

    let maskMenuItems = $derived<ContextMenuItem[]>([
        { label: maskEnabled ? 'Disable mask' : 'Enable mask', onclick: toggleMaskEnabled },
        { label: isMaskIsolated ? 'Hide mask' : 'Show mask', onclick: toggleShowMask },
        { label: 'Apply mask', disabled: !editable, onclick: applyMask },
        { label: 'Delete mask', disabled: !editable, onclick: removeMask },
    ]);

    let layerMenuItems = $derived.by<ContextMenuItem[]>(() => {
        const items: ContextMenuItem[] = [
            { label: dupLabel, onclick: menuDuplicate },
        ];
        if (!isMulti) {
            items.push({ label: 'Add mask', disabled: !canAddMask, onclick: menuAddMask });
        }
        items.push({
            label: mergeLabel,
            disabled: !isMulti && (!canMergeDownForThis || !editable),
            onclick: menuMerge,
        });
        if (!isMulti && hasMask) {
            items.push({ label: 'Flatten', disabled: !editable, onclick: menuFlatten });
        }
        items.push({ separator: true });
        items.push({
            label: deleteLabel,
            disabled: !isMulti && !editable,
            onclick: menuDelete,
        });
        return items;
    });

    // Structural menu items dispatch WITHOUT `ctx.layerId` — the action
    // handler reads `app.selectedLayerIds` (the right-click handler above
    // guarantees the clicked row is in the selection). This is what
    // makes "Delete 3 Layers" actually delete 3 layers; the v1 attempt
    // passed `{ layerId: layer.id }` and silently demoted every action
    // to single-layer.

    function menuDuplicate() {
        actions.dispatch('duplicateLayer');
        onupdate();
    }

    function menuMerge() {
        // `mergeDown` is selection-aware: with ≥2 selected it bakes the
        // selection via merge_layers; with 1, it does the classic
        // single-layer merge-down. Guard only the single-layer case
        // where there's no sibling below.
        if (!isMulti && !canMergeDownForThis) return;
        actions.dispatch('mergeDown');
        onupdate();
    }

    function menuFlatten() {
        if (!hasMask) return;
        actions.dispatch('flatten');
        onupdate();
    }

    function menuAddMask() {
        if (!canAddMask) return;
        actions.dispatch('addMask');
        onupdate();
    }

    function menuDelete() {
        if (!editable && !isMulti) return;
        actions.dispatch('deleteLayer');
        onupdate();
    }

    function toggleMaskEnabled() {
        if (app.engine && maskModifier !== null) {
            app.engine.post('set_layer_visible', { id: maskModifier.id, visible: !maskEnabled });
            onupdate();
        }
    }

    function toggleShowMask() {
        if (app.engine && maskModifier !== null) {
            const next = isMaskIsolated ? 0 : maskModifier.id;
            app.engine.post('set_isolated_node', { id: next });
            app.isolatedNodeId = next === 0 ? null : next;
            onupdate();
        }
    }

    function applyMask() {
        if (app.engine) {
            app.engine.post('apply_mask', { id: layer.id });
            onupdate();
        }
    }

    function removeMask() {
        if (app.engine) {
            app.engine.post('remove_mask', { id: layer.id });
            onupdate();
        }
    }

    function startRename() {
        if (!layer.canRename) return;
        if (!editable) return;
        editing = true;
        requestAnimationFrame(() => editInput?.focus());
    }

    function finishRename() {
        editing = false;
        if (app.engine && editInput) {
            app.engine.post('set_layer_name', { id: layer.id, name: editInput.value });
            onupdate();
        }
    }

    let draggable = $state(true);

    function onDragStart(e: DragEvent) {
        // Grabbed row IS in selection → drag the whole set. Grabbed row
        // is NOT in selection → drag only it, and replace the selection
        // with just it (focus commits to what the user grabbed).
        const ids = app.isSelected(layer.id)
            ? [...app.selectedLayerIds]
            : [layer.id];
        if (!app.isSelected(layer.id)) {
            app.selectLayer(layer.id);
        }
        e.dataTransfer?.setData(
            'application/x-darkly-layers',
            JSON.stringify(ids),
        );
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

    async function onDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        dropPos = 'none';
        const payload = e.dataTransfer?.getData('application/x-darkly-layers');
        const engine = app.engine;
        if (!payload || !engine) return;
        let ids: number[];
        try { ids = JSON.parse(payload) as number[]; } catch { return; }
        if (!Array.isArray(ids) || ids.length === 0) return;
        // Dropping the dragged set onto one of its own members is a no-op
        // — the engine would reject it as self-referential anyway.
        if (ids.includes(layer.id)) return;

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        const where = ratio < 0.5 ? 'after' : 'before';

        try {
            const { skipped } = await engine.send('move_layers', {
                ids, target_type: where, target_id: layer.id,
            });
            if (skipped > 0) {
                toast.show('info', `${skipped} locked layer${skipped === 1 ? '' : 's'} skipped`);
            }
        } catch (e: any) {
            toast.show('error', e.message ?? String(e));
        }
        onupdate();
    }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
    class="layer-item"
    class:active={isActive}
    class:selected={isSelected}
    class:drop-above={dropPos === 'above'}
    class:drop-below={dropPos === 'below'}
    onclick={onLayerClick}
    ondblclick={startRename}
    oncontextmenu={onLayerContextMenu}
    role="button"
    tabindex="-1"
    draggable={draggable && editable ? 'true' : 'false'}
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
        title={tooltipForAction('Toggle visibility', 'toggleVisibility')}
    >
        <Icon name={layer.visible ? 'fa6-solid:eye' : 'fa6-solid:eye-slash'} />
    </button>

    {#if layer.hasThumbnail && layerThumb}
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
    {:else if layer.icon}
        <span
            class="thumb void-thumb"
            class:thumb-active={isActive && !isEditingMask}
            title={layer.kindName}
        >
            <Icon name={layer.icon} />
        </span>
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

    <button
        class="lock-btn"
        class:locked={layer.locked}
        use:bindingSite={{ name: 'layerLock', ctx: () => ({ layerId: layer.id }) }}
        onclick={toggleLock}
        onpointerdown={(e: PointerEvent) => { e.stopPropagation(); draggable = false; }}
        onpointerup={() => { draggable = true; }}
        onpointerleave={() => { draggable = true; }}
        title={tooltipForAction(layer.locked ? 'Unlock layer' : 'Lock layer', 'toggleLock')}
    >
        <Icon name={layer.locked ? 'fa6-solid:lock' : 'fa6-solid:lock-open'} />
    </button>
</div>

<!-- Duplicate stays enabled even when locked: it reads the source and creates
     a new layer rather than mutating the locked one. -->
{#if showMaskMenu}
    <ContextMenu
        x={maskMenuX}
        y={maskMenuY}
        items={maskMenuItems}
        onclose={() => (showMaskMenu = false)}
    />
{/if}

{#if showLayerMenu}
    <ContextMenu
        x={layerMenuX}
        y={layerMenuY}
        items={layerMenuItems}
        onclose={() => (showLayerMenu = false)}
    />
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

    .layer-item.selected {
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

    .lock-btn {
        width: 24px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-dim);
        cursor: pointer;
        font-size: 12px;
        flex-shrink: 0;
        border-radius: 4px;
        transition: color 0.1s;
    }
    .lock-btn:hover { color: var(--text); }
    .lock-btn.locked { color: var(--text); }

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

    .void-thumb {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        font-size: 14px;
        color: var(--text-muted);
        cursor: default;
    }
    .void-thumb.thumb-active {
        color: var(--accent);
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
</style>
