<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail } from './thumbnails.svelte';
    import { actions } from '../../actions/registry';
    import { bindingSite } from '../../actions/binding_site';
    import { tooltipForAction } from '../../config/store.svelte';
    import { toast } from '../../state/toast.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';
    import Icon from '../../icons/Icon.svelte';
    import ContextMenu, { type ContextMenuItem } from '../ContextMenu.svelte';
    import MaskChainControl from './MaskChainControl.svelte';
    import { layerDropTarget } from './dropTarget.svelte';

    interface Modifier {
        id: number; kind: string; name: string; visible: boolean; locked: boolean;
        linkedToHost: boolean; editable: boolean;
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
            app.engine.api.setGroupCollapsed({ id: group.id, collapsed: !group.collapsed });
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
            app.engine.api.setLayerName({ id: group.id, name: editInput.value });
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
    }

    let maskMenuItems = $derived<ContextMenuItem[]>([
        { label: maskEnabled ? 'Disable mask' : 'Enable mask', onclick: toggleMaskEnabled },
        { label: isMaskIsolated ? 'Hide mask' : 'Show mask', onclick: toggleShowMask },
        { label: 'Mask to Selection', onclick: menuMaskToSelection },
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
        if (!isMulti) {
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
            app.engine.api.setLayerVisible({ id: maskModifier.id, visible: !maskEnabled });
            onupdate();
        }
    }

    function toggleShowMask() {
        if (app.engine && maskModifier !== null) {
            const next = isMaskIsolated ? null : maskModifier.id;
            void app.setIsolatedNode(next);
            onupdate();
        }
    }

    function removeMask() {
        if (app.engine) {
            app.engine.api.removeMask({ id: group.id });
            onupdate();
        }
    }

    // Routes through the action (not a direct api call like the siblings
    // above) so the mask-menu entry and the maskThumb $mod+click gesture
    // share one home for the op.
    function menuMaskToSelection() {
        if (maskModifier === null) return;
        actions.dispatch('maskToSelection', { maskId: maskModifier.id });
        onupdate();
    }

</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="layer-group" style:--depth={depth}>
    <div
        class="group-header"
        class:active={isActive}
        class:selected={isSelected}
        onclick={onLayerClick}
        ondblclick={startRename}
        oncontextmenu={onLayerContextMenu}
        role="button"
        tabindex="-1"
        draggable={editable ? 'true' : 'false'}
        use:layerDropTarget={{
            rowId: group.id,
            isGroup: true,
            draggable: editable,
            onupdate,
        }}
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

        {#if maskModifier}
            <MaskChainControl
                mask={maskModifier}
                thumbnail={maskThumb}
                active={isEditingMask}
                enabled={maskEnabled}
                onselect={clickMaskThumb}
                oncontextmenu={onMaskContextMenu}
                {onupdate}
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

<!-- Duplicate stays enabled when locked: it reads the source, not mutates it. -->
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

</style>
