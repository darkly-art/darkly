<script lang="ts">
    /**
     * One row of the layer panel, for any kind of node.
     *
     * There used to be two of these, `LayerItem` and `LayerGroup`, and they
     * were the same component twice: seventeen identical handlers, the same
     * mask sub-row, and a hundred lines of CSS that differed in a gap value.
     *
     * One row serves every kind because the row never asks what kind it is
     * looking at. Every question it needs answered is a per-kind capability
     * flag the engine already puts on `LayerInfo` (`paintable`, `canHaveMask`,
     * `canRename`, `hasThumbnail`, `canBecomeSmartObject`, `icon`, `kindName`),
     * emitted on the group variant exactly as on the others. The one
     * structural question, "does this hold other rows", goes through
     * `isContainer` so the drop model and this component cannot disagree about
     * it.
     *
     * Adding a layer kind therefore needs no edit here: declare the
     * capabilities in its `LayerKindRegistration` and the row follows.
     */
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail, THUMB_SIZE } from './thumbnails.svelte';
    import { bindingSite } from '../../actions/binding_site';
    import { actions } from '../../actions/registry';
    import { tooltipForAction } from '../../config/store.svelte';
    import Icon from '../../icons/Icon.svelte';
    import ContextMenu, { type ContextMenuItem } from '../ContextMenu.svelte';
    import { flattenOffer, smartObjectOffer } from './menu_offers';
    import { layerDropTarget } from './dropTarget.svelte';
    import MaskChainControl from './MaskChainControl.svelte';
    import { hasSiblingBelow, isContainer } from '../../state/layerTree';
    import { maskOf, type RowNode } from './rowNode';
    import LayerRows from './LayerRows.svelte';

    let { node, depth = 0, onupdate }: {
        node: RowNode;
        depth?: number;
        onupdate: () => void;
    } = $props();

    // `editable` is the effective form (false when this node OR any ancestor is
    // locked, mirroring `Document::is_node_editable`) and gates interaction;
    // `locked` is the node's own flag and drives the icon.
    let editable = $derived(node.editable !== false);
    let paintable = $derived(node.paintable !== false);
    let container = $derived(isContainer(node));
    let children = $derived(container && 'children' in node ? node.children : []);
    let collapsed = $derived(container && 'collapsed' in node ? node.collapsed : false);

    let maskModifier = $derived(maskOf(node));
    let hasMask = $derived(maskModifier !== null);
    let maskEnabled = $derived(maskModifier?.visible ?? true);
    let isMaskIsolated = $derived(
        maskModifier !== null && app.isolatedNodeId === maskModifier.id,
    );
    // The mask is the active edit target whenever the active node id IS the
    // mask modifier id (no session redirect).
    let isEditingMask = $derived(
        maskModifier !== null && app.activeLayerId === maskModifier.id,
    );

    let isActive = $derived(app.activeLayerId === node.id);
    let isSelected = $derived(app.isSelected(node.id));
    let selectionSize = $derived(app.selectedLayerIds.size);
    let isMulti = $derived(selectionSize > 1);

    // The generic noun stays generic: "Delete Layer" is what the panel has
    // always said for anything that is not a group, and a per-kind noun
    // ("Delete Raster Layer") is wordier without being clearer.
    let noun = $derived(container ? 'Group' : 'Layer');
    let deleteLabel = $derived(isMulti ? `Delete ${selectionSize} Layers` : `Delete ${noun}`);
    let dupLabel = $derived(isMulti ? `Duplicate ${selectionSize} Layers` : `Duplicate ${noun}`);
    let mergeLabel = $derived(isMulti ? `Merge ${selectionSize} Layers` : 'Merge Down');

    let canMergeDownForThis = $derived(hasSiblingBelow(app.layerTree, node.id));
    let canAddMask = $derived(Boolean(node.canHaveMask) && !hasMask && editable);
    let flattenLabel = $derived(flattenOffer({ paintable, hasMask, isContainer: container }));
    let offersSmartObject = $derived(smartObjectOffer(node, isMulti));

    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let draggable = $state(true);

    let nodeThumb = $derived(node.hasThumbnail && app.engine ? getNodeThumbnail(node.id) : '');
    let maskThumb = $derived(maskModifier !== null && app.engine ? getNodeThumbnail(maskModifier.id) : '');

    let showMaskMenu = $state(false);
    let maskMenuX = $state(0);
    let maskMenuY = $state(0);

    let showLayerMenu = $state(false);
    let layerMenuX = $state(0);
    let layerMenuY = $state(0);

    // Chord dispatch is owned by `use:bindingSite` on each preview element
    // below: it intercepts modifier+click in capture phase and dispatches
    // against its named site. These onclick handlers are the no-chord fallback.
    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleVisibility', { layerId: node.id });
        onupdate();
    }

    function toggleLock(e: MouseEvent) {
        e.stopPropagation();
        actions.dispatch('toggleLock', { layerId: node.id });
        onupdate();
    }

    function toggleCollapsed(e: MouseEvent) {
        e.stopPropagation();
        if (!app.engine) return;
        app.engine.api.setGroupCollapsed({ id: node.id, collapsed: !collapsed });
        onupdate();
    }

    /** The row body has no chord bindings; modifier+click is reserved for the
     *  previews. Plain / ctrl / shift dispatch is shared with every other row
     *  through `app.handleLayerRowClick`. */
    function onLayerClick(e: MouseEvent) {
        app.handleLayerRowClick(node.id, e);
    }

    function clickNodeThumb(e: MouseEvent) {
        e.stopPropagation();
        app.selectLayer(node.id);
    }

    /** Activating the mask = setting the active node id to the modifier's id.
     *  There is no separate "edit mask" redirect. */
    function clickMaskThumb(e: MouseEvent) {
        e.stopPropagation();
        if (maskModifier === null) return;
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
        // If the right-clicked row is already in the multi-selection, keep the
        // selection intact, since the menu acts on the whole set. If it is not,
        // replace the selection with just this row (Photoshop / GIMP
        // behaviour). Either way the menu and every action it dispatches
        // operate on a selection that always includes the right-clicked row.
        if (!app.isSelected(node.id)) {
            app.selectLayer(node.id);
        }
        layerMenuX = e.clientX;
        layerMenuY = e.clientY;
        showLayerMenu = true;
    }

    let maskMenuItems = $derived<ContextMenuItem[]>([
        { label: maskEnabled ? 'Disable mask' : 'Enable mask', onclick: toggleMaskEnabled },
        { label: isMaskIsolated ? 'Hide mask' : 'Show mask', onclick: toggleShowMask },
        { label: 'Mask to Selection', onclick: menuMaskToSelection },
        // `apply_mask` bakes the mask into the host's pixels, so it needs
        // pixels to bake into. `paintable` is the engine's own predicate for
        // that, so a container or a generated-pixel row offers the entry
        // disabled rather than enabled and inert.
        { label: 'Apply mask', disabled: !editable || !paintable, onclick: applyMask },
        { label: 'Delete mask', disabled: !editable, onclick: removeMask },
    ]);

    let layerMenuItems = $derived.by<ContextMenuItem[]>(() => {
        const items: ContextMenuItem[] = [
            { label: dupLabel, onclick: menuDuplicate },
        ];
        if (!isMulti) {
            items.push({ label: 'Add mask', disabled: !canAddMask, onclick: menuAddMask });
            items.push({
                label: 'Alpha to Selection',
                disabled: !node.hasThumbnail,
                onclick: menuAlphaToSelection,
            });
        }
        items.push({
            label: mergeLabel,
            disabled: !isMulti && (!canMergeDownForThis || !editable),
            onclick: menuMerge,
        });
        if (!isMulti && flattenLabel) {
            items.push({ label: flattenLabel, disabled: !editable, onclick: menuFlatten });
        }
        // Sits next to Flatten: both swap the layer for a different
        // representation of the same picture, in opposite directions. One bakes
        // it down to pixels, the other keeps the pixels as a source you can
        // keep rescaling.
        if (offersSmartObject) {
            items.push({ label: 'Convert to Smart Object', onclick: menuConvertToSmartObject });
        }
        items.push({ separator: true });
        items.push({ label: deleteLabel, disabled: !isMulti && !editable, onclick: menuDelete });
        return items;
    });

    // Structural menu items dispatch WITHOUT `ctx.layerId`: the action handler
    // reads `app.selectedLayerIds` (the right-click handler above guarantees
    // the clicked row is in the selection). This is what makes "Delete 3
    // Layers" actually delete 3 layers; passing `{ layerId: node.id }` would
    // silently demote every action to single-layer.

    function menuDuplicate() {
        actions.dispatch('duplicateLayer');
        onupdate();
    }

    function menuMerge() {
        // `mergeDown` is selection-aware: with two or more selected it bakes
        // the selection via merge_layers; with one, it does the classic
        // single-layer merge-down. Guard only the single-layer case where
        // there is no sibling below.
        if (!isMulti && !canMergeDownForThis) return;
        actions.dispatch('mergeDown');
        onupdate();
    }

    function menuFlatten() {
        if (!flattenLabel) return;
        actions.dispatch('flatten');
        onupdate();
    }

    /** Dispatched with an explicit `layerId`: this one acts on the row that was
     *  right-clicked, not on the selection, because the entry is offered for a
     *  single row only (see `smartObjectOffer`). */
    function menuConvertToSmartObject() {
        if (!offersSmartObject) return;
        actions.dispatch('convertLayerToSmartObject', { layerId: node.id });
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
            void app.setIsolatedNode(isMaskIsolated ? null : maskModifier.id);
            onupdate();
        }
    }

    function applyMask() {
        if (app.engine) {
            app.engine.api.applyMask({ id: node.id });
            onupdate();
        }
    }

    /** Routes through the action (not a direct api call like the siblings
     *  above) so the mask-menu entry and the maskThumb $mod+click gesture share
     *  one home for the op. */
    function menuMaskToSelection() {
        if (maskModifier === null) return;
        actions.dispatch('maskToSelection', { maskId: maskModifier.id });
        onupdate();
    }

    /** Same seam as the mask entry above: the menu row and the layerThumb
     *  $mod+click gesture both land on the action. */
    function menuAlphaToSelection() {
        if (!node.hasThumbnail) return;
        actions.dispatch('alphaToSelection', { layerId: node.id });
        onupdate();
    }

    function removeMask() {
        if (app.engine) {
            app.engine.api.removeMask({ id: node.id });
            onupdate();
        }
    }

    function startRename() {
        if (!node.canRename || !editable) return;
        editing = true;
        requestAnimationFrame(() => editInput?.focus());
    }

    function finishRename() {
        editing = false;
        if (app.engine && editInput) {
            app.engine.api.setLayerName({ id: node.id, name: editInput.value });
            onupdate();
        }
    }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
    class="layer-row"
    class:container
    class:active={isActive}
    class:selected={isSelected}
    onclick={onLayerClick}
    ondblclick={startRename}
    oncontextmenu={onLayerContextMenu}
    role="button"
    tabindex="-1"
    draggable={draggable && editable ? 'true' : 'false'}
    use:layerDropTarget={{
        rowId: node.id,
        isGroup: container,
        draggable: draggable && editable,
        onupdate,
    }}
    style:padding-left="{8 + depth * 16}px"
>
    <!-- The eye and lock buttons suppress the row drag while pressed, so
         pressing them never starts one. -->
    <button
        class="vis-btn"
        class:hidden={!node.visible}
        use:bindingSite={{ name: 'layerEye', ctx: () => ({ layerId: node.id }) }}
        onclick={toggleVisibility}
        onpointerdown={(e: PointerEvent) => { e.stopPropagation(); draggable = false; }}
        onpointerup={() => { draggable = true; }}
        onpointerleave={() => { draggable = true; }}
        title={tooltipForAction('Toggle visibility', 'toggleVisibility')}
    >
        <Icon name={node.visible ? 'fa6-solid:eye' : 'fa6-solid:eye-slash'} />
    </button>

    {#if container}
        <button class="collapse-btn" onclick={toggleCollapsed} title="Toggle collapsed">
            <Icon name={collapsed ? 'fa6-solid:chevron-right' : 'fa6-solid:chevron-down'} />
        </button>
    {/if}

    {#if node.hasThumbnail && nodeThumb}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <img
            class="thumb"
            class:thumb-active={isActive && !isEditingMask}
            src={nodeThumb}
            alt="layer"
            width={THUMB_SIZE}
            height={THUMB_SIZE}
            draggable="false"
            use:bindingSite={{ name: 'layerThumb', ctx: () => ({ layerId: node.id }) }}
            onclick={clickNodeThumb}
        />
    {:else if node.icon}
        <span
            class="thumb kind-thumb"
            class:thumb-active={isActive && !isEditingMask}
            title={node.kindName}
        >
            <Icon name={node.icon} />
        </span>
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

    {#if editing}
        <input
            class="name-input"
            bind:this={editInput}
            value={node.name}
            onblur={finishRename}
            onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') finishRename(); }}
            onclick={(e: MouseEvent) => e.stopPropagation()}
        />
    {:else}
        <span class="layer-name">{node.name}</span>
    {/if}

    <button
        class="lock-btn"
        class:locked={node.locked}
        use:bindingSite={{ name: 'layerLock', ctx: () => ({ layerId: node.id }) }}
        onclick={toggleLock}
        onpointerdown={(e: PointerEvent) => { e.stopPropagation(); draggable = false; }}
        onpointerup={() => { draggable = true; }}
        onpointerleave={() => { draggable = true; }}
        title={tooltipForAction(node.locked ? 'Unlock layer' : 'Lock layer', 'toggleLock')}
    >
        <Icon name={node.locked ? 'fa6-solid:lock' : 'fa6-solid:lock-open'} />
    </button>
</div>

{#if container && !collapsed}
    <LayerRows nodes={children} depth={depth + 1} {onupdate} />
{/if}

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
    .layer-row {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px;
        cursor: pointer;
        user-select: none;
        border-left: 2px solid transparent;
    }

    .layer-row:focus,
    .layer-row:focus-visible {
        outline: none;
    }

    .layer-row:hover {
        background: var(--bg-hover);
    }

    .layer-row.selected {
        background: var(--bg-active);
    }

    .layer-row.active {
        border-left-color: var(--accent);
        background: var(--bg-active);
    }

    /* A container's name carries the extra weight; everything else about the
       row is identical to a leaf's. */
    .layer-row.container .layer-name {
        font-weight: 600;
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

    .vis-btn,
    .lock-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 18px;
        flex-shrink: 0;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 11px;
    }

    .vis-btn.hidden,
    .lock-btn.locked {
        color: var(--text-dim);
    }

    .thumb {
        width: 32px;
        height: 32px;
        border: 2px solid var(--text-dim);
        border-radius: var(--radius-sm);
        flex-shrink: 0;
        cursor: pointer;
        image-rendering: pixelated;
        background: var(--thumb-bg);
    }

    .thumb.thumb-active {
        border-color: var(--accent);
    }

    /* Kinds with no live thumbnail (group, void, filter) render their
       registration's icon in the same box. */
    .kind-thumb {
        display: flex;
        align-items: center;
        justify-content: center;
        color: var(--text-muted);
        font-size: 14px;
        image-rendering: auto;
    }

    .layer-name {
        flex: 1;
        min-width: 0;
        font-size: 12px;
        color: var(--text);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .name-input {
        flex: 1;
        min-width: 0;
        font-size: 12px;
        font-family: inherit;
        color: var(--text);
        background: var(--bg);
        border: 1px solid var(--accent);
        border-radius: var(--radius-sm);
        padding: 1px 4px;
        outline: none;
    }
</style>
