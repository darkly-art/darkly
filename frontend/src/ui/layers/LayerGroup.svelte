<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail, THUMB_SIZE } from './thumbnails.svelte';
    import { actions } from '../../actions/registry';
    import { bindingSite } from '../../actions/binding_site';
    import { tooltipForAction } from '../../config/store.svelte';
    import { toast } from '../../state/toast.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';
    import Icon from '../../icons/Icon.svelte';

    interface Modifier {
        id: number; kind: string; name: string; visible: boolean; locked: boolean;
    }

    let { group, depth = 0, onupdate }: {
        group: {
            type: 'group'; id: number; name: string; visible: boolean;
            locked?: boolean;
            // See LayerItem — `locked` is the node's own flag, `editable`
            // is the effective (ancestor-aware) form used to gate edits.
            editable?: boolean;
            // Per-kind mask capability (see LayerKindRegistration) — gates the
            // add-mask control without branching on the kind.
            canHaveMask?: boolean;
            collapsed: boolean; passthrough: boolean; opacity: number;
            blendMode: string; children: any[];
            modifiers?: Modifier[];
        };
        depth?: number;
        onupdate: () => void;
    } = $props();

    let editable = $derived(group.editable !== false);

    let maskModifier = $derived<Modifier | null>(
        group.modifiers?.find((m) => m.kind === 'mask') ?? null,
    );
    let hasMask = $derived(maskModifier !== null);
    let maskEnabled = $derived(maskModifier?.visible ?? true);
    let isMaskIsolated = $derived(
        maskModifier !== null && app.isolatedNodeId === maskModifier.id,
    );

    let isActive = $derived(app.activeLayerId === group.id);
    let isSelected = $derived(app.isSelected(group.id));
    let selectionSize = $derived(app.selectedLayerIds.size);
    let isMulti = $derived(selectionSize > 1);
    let deleteLabel = $derived(isMulti ? `Delete ${selectionSize} Layers` : 'Delete Group');
    let dupLabel = $derived(isMulti ? `Duplicate ${selectionSize} Layers` : 'Duplicate Group');
    let mergeLabel = $derived(isMulti ? `Merge ${selectionSize} Layers` : 'Merge Down');
    let isEditingMask = $derived(
        maskModifier !== null && app.activeLayerId === maskModifier.id,
    );
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below' | 'into'>('none');

    let maskThumb = $derived(maskModifier !== null && app.engine ? getNodeThumbnail(maskModifier.id) : '');
    let showMaskMenu = $state(false);
    let maskMenuX = $state(0);
    let maskMenuY = $state(0);

    let showLayerMenu = $state(false);
    let layerMenuX = $state(0);
    let layerMenuY = $state(0);

    /// Same predicate as LayerItem — kept colocated rather than pulled into
    /// a shared helper for one walk's worth of code.
    function siblingBelowExists(nodes: any[], id: number): boolean {
        for (const n of nodes) {
            if (n.id === id) return false;
            if (n.children) {
                const idx = n.children.findIndex((c: any) => c.id === id);
                if (idx >= 0) return idx < n.children.length - 1;
                if (siblingBelowExists(n.children, id)) return true;
            }
        }
        return false;
    }

    let canMergeDownForThis = $derived.by(() => {
        const topIdx = app.layerTree.findIndex((n: any) => n.id === group.id);
        if (topIdx >= 0) return topIdx < app.layerTree.length - 1;
        return siblingBelowExists(app.layerTree, group.id);
    });

    let canAddMask = $derived(Boolean(group.canHaveMask) && !hasMask && editable);

    // Chord dispatch is owned by `use:bindingSite` on each preview element
    // below — `bindingSite` intercepts modifier+click in capture phase
    // and dispatches against its named site. These onclick handlers are
    // the no-chord fallback.
    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleVisibility', { layerId: group.id });
        onupdate();
    }

    function toggleLock(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleLock', { layerId: group.id });
        onupdate();
    }

    function toggleCollapsed(e: MouseEvent) {
        e.stopPropagation();
        if (app.engine) {
            app.engine.post('set_group_collapsed', { id: group.id, collapsed: !group.collapsed });
            onupdate();
        }
    }

    function onLayerClick(e: MouseEvent) {
        // The group-header body has no bindings — modifier+click is
        // reserved for the previews. Plain / ctrl / shift dispatch is
        // shared with LayerItem via app.handleLayerRowClick.
        app.handleLayerRowClick(group.id, e);
    }

    function startRename() {
        if (!editable) return;
        editing = true;
        requestAnimationFrame(() => editInput?.focus());
    }

    function finishRename() {
        editing = false;
        if (app.engine && editInput) {
            app.engine.post('set_layer_name', { id: group.id, name: editInput.value });
            onupdate();
        }
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
        // Mirror LayerItem: only replace the selection if the right-
        // clicked row isn't already part of it. Otherwise the menu
        // operates on the whole selected set.
        if (!app.isSelected(group.id)) {
            app.selectLayer(group.id);
        }
        layerMenuX = e.clientX;
        layerMenuY = e.clientY;
        showLayerMenu = true;
        const close = () => { showLayerMenu = false; document.removeEventListener('click', close); };
        requestAnimationFrame(() => document.addEventListener('click', close));
    }

    // Structural menu items dispatch WITHOUT `ctx.layerId` — the action
    // handler reads `app.selectedLayerIds` directly. See LayerItem.svelte
    // for the same pattern and the rationale.

    function menuDuplicate() {
        actions.dispatch('duplicateLayer');
        onupdate();
    }

    function menuMerge() {
        // `mergeDown` is selection-aware (see LayerItem.svelte's menuMerge).
        if (!isMulti && !canMergeDownForThis) return;
        actions.dispatch('mergeDown');
        onupdate();
    }

    function menuFlatten() {
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

    function removeMask() {
        if (app.engine) {
            app.engine.post('remove_mask', { id: group.id });
            onupdate();
        }
    }

    function onDragStart(e: DragEvent) {
        const ids = app.isSelected(group.id)
            ? [...app.selectedLayerIds]
            : [group.id];
        if (!app.isSelected(group.id)) {
            app.selectLayer(group.id);
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
        if (ratio < 0.25) {
            dropPos = 'above';
        } else if (ratio > 0.75) {
            dropPos = 'below';
        } else {
            dropPos = 'into';
        }
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
        const pos = dropPos;
        dropPos = 'none';
        const payload = e.dataTransfer?.getData('application/x-darkly-layers');
        const engine = app.engine;
        if (!payload || !engine) return;
        let ids: number[];
        try { ids = JSON.parse(payload) as number[]; } catch { return; }
        if (!Array.isArray(ids) || ids.length === 0) return;
        if (ids.includes(group.id)) return;

        const where = pos === 'above' ? 'after'
            : pos === 'below' ? 'before'
            : 'into_top';

        try {
            const { skipped } = await engine.send('move_layers', {
                ids, target_type: where, target_id: group.id,
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
<div class="layer-group" style:--depth={depth}>
    <div
        class="group-header"
        class:active={isActive}
        class:selected={isSelected}
        class:drop-above={dropPos === 'above'}
        class:drop-below={dropPos === 'below'}
        class:drop-into={dropPos === 'into'}
        onclick={onLayerClick}
        ondblclick={startRename}
        oncontextmenu={onLayerContextMenu}
        role="button"
        tabindex="-1"
        draggable={editable ? 'true' : 'false'}
        ondragstart={onDragStart}
        ondragover={onDragOver}
        ondragleave={onDragLeave}
        ondrop={onDrop}
        ondragend={() => { dropPos = 'none'; }}
        style:padding-left="{8 + depth * 16}px"
    >
        <button
            class="vis-btn"
            class:hidden={!group.visible}
            use:bindingSite={{ name: 'layerEye', ctx: () => ({ layerId: group.id }) }}
            onclick={toggleVisibility}
            onpointerdown={(e: PointerEvent) => { e.stopPropagation(); }}
            title={tooltipForAction('Toggle visibility', 'toggleVisibility')}
        >
            <Icon name={group.visible ? 'fa6-solid:eye' : 'fa6-solid:eye-slash'} />
        </button>

        <button class="collapse-btn" onclick={toggleCollapsed} title="Toggle collapsed">
            <Icon name={group.collapsed ? 'fa6-solid:chevron-right' : 'fa6-solid:chevron-down'} />
        </button>

        <Icon name={group.collapsed ? 'fa6-solid:folder' : 'fa6-solid:folder-open'} class="folder-icon" />

        {#if editing}
            <input
                class="name-input"
                bind:this={editInput}
                value={group.name}
                onblur={finishRename}
                onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') finishRename(); }}
                onclick={(e: MouseEvent) => e.stopPropagation()}
            />
        {:else}
            <span class="group-name">{group.name}</span>
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

        <button
            class="lock-btn"
            class:locked={group.locked}
            use:bindingSite={{ name: 'layerLock', ctx: () => ({ layerId: group.id }) }}
            onclick={toggleLock}
            onpointerdown={(e: PointerEvent) => { e.stopPropagation(); }}
            title={tooltipForAction(group.locked ? 'Unlock group' : 'Lock group', 'toggleLock')}
        >
            <Icon name={group.locked ? 'fa6-solid:lock' : 'fa6-solid:lock-open'} />
        </button>
    </div>

{#if showMaskMenu}
    <div class="mask-menu" style:left="{maskMenuX}px" style:top="{maskMenuY}px">
        <button onclick={toggleMaskEnabled}>
            {maskEnabled ? 'Disable mask' : 'Enable mask'}
        </button>
        <button onclick={toggleShowMask}>
            {isMaskIsolated ? 'Hide mask' : 'Show mask'}
        </button>
        <button onclick={removeMask} disabled={!editable}>Delete mask</button>
    </div>
{/if}

{#if showLayerMenu}
    <div class="layer-menu" style:left="{layerMenuX}px" style:top="{layerMenuY}px">
        <!-- Duplicate doesn't mutate the locked node — allowed. -->
        <button onclick={menuDuplicate}>{dupLabel}</button>
        {#if !isMulti}
            <button onclick={menuAddMask} disabled={!canAddMask}>
                Add mask
            </button>
        {/if}
        <button onclick={menuMerge} disabled={!isMulti && (!canMergeDownForThis || !editable)}>
            {mergeLabel}
        </button>
        {#if !isMulti}
            <button onclick={menuFlatten} disabled={!editable}>Flatten</button>
        {/if}
        <div class="layer-menu-sep"></div>
        <button onclick={menuDelete} disabled={!isMulti && !editable}>
            {deleteLabel}
        </button>
    </div>
{/if}

    {#if !group.collapsed}
        <div class="group-children">
            {#each group.children as child (child.id)}
                {#if child.type === 'group'}
                    <LayerGroup group={child} depth={depth + 1} {onupdate} />
                {:else}
                    <LayerItem layer={child} depth={depth + 1} {onupdate} />
                {/if}
            {/each}
        </div>
    {/if}
</div>

<style>
    .group-header {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 12px;
        cursor: pointer;
        min-height: 28px;
        position: relative;
        transition: background 0.1s;
        user-select: none;
    }

    .group-header:focus,
    .group-header:focus-visible {
        outline: none;
    }

    .group-header:hover {
        background: var(--bg-hover);
    }

    .group-header.selected {
        background: var(--bg-hover);
    }

    .group-header.active {
        background: var(--bg-active);
    }

    .group-header.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .group-header.drop-below::after {
        content: '';
        position: absolute;
        bottom: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .group-header.drop-into {
        outline: 1px solid var(--accent);
        outline-offset: -1px;
    }

    .collapse-btn {
        width: 16px;
        height: 16px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 9px;
        flex-shrink: 0;
        transition: transform 0.15s;
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

    .group-header :global(.folder-icon) {
        color: var(--text-muted);
        font-size: 12px;
        width: 14px;
        text-align: center;
        flex-shrink: 0;
    }

    .group-name {
        flex: 1;
        font-size: 12px;
        color: var(--text);
        font-weight: 600;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
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
    }

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
    .thumb-active { border-color: var(--accent); }
    .mask-disabled { opacity: 0.4; }

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
        padding: 4px 12px;
        text-align: left;
        cursor: pointer;
        font-size: 12px;
        white-space: nowrap;
    }
    .mask-menu button:hover { background: var(--bg-hover); }

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
    .layer-menu button:hover:not(:disabled) { background: var(--bg-hover); }
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
