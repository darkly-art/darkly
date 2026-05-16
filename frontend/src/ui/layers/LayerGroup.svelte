<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { getNodeThumbnail, THUMB_SIZE } from './thumbnails';
    import { dispatchClick } from '../../actions/triggers';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';

    interface Modifier {
        id: number; kind: string; name: string; visible: boolean; locked: boolean;
    }

    let { group, depth = 0, onupdate }: {
        group: {
            type: 'group'; id: number; name: string; visible: boolean;
            collapsed: boolean; passthrough: boolean; opacity: number;
            blendMode: string; children: any[];
            modifiers?: Modifier[];
        };
        depth?: number;
        onupdate: () => void;
    } = $props();

    let maskModifier = $derived<Modifier | null>(
        group.modifiers?.find((m) => m.kind === 'mask') ?? null,
    );
    let hasMask = $derived(maskModifier !== null);
    let maskEnabled = $derived(maskModifier?.visible ?? true);
    let isMaskIsolated = $derived(
        maskModifier !== null && app.isolatedNodeId === maskModifier.id,
    );

    let isActive = $derived(app.activeLayerId === group.id);
    let isEditingMask = $derived(
        maskModifier !== null && app.activeLayerId === maskModifier.id,
    );
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below' | 'into'>('none');

    let maskThumb = $derived(maskModifier !== null && app.handle ? getNodeThumbnail(maskModifier.id) : '');
    let showMaskMenu = $state(false);
    let maskMenuX = $state(0);
    let maskMenuY = $state(0);

    // Sub-region click handlers fire site-specific bindings only — no
    // cross-site fallback. Modifier+click on the group's mask thumb
    // dispatches with the mask's id so `isolateLayer` solos the mask, not
    // the group.
    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (dispatchClick('layerEye', e, { layerId: group.id })) {
            onupdate();
            return;
        }
        if (app.handle) {
            app.handle.set_layer_visible(group.id, !group.visible);
            onupdate();
        }
    }

    function toggleCollapsed(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.set_group_collapsed(group.id, !group.collapsed);
            onupdate();
        }
    }

    function onLayerClick() {
        // The group-header body has no bindings — modifier+click is
        // reserved for the previews. Plain click selects.
        app.selectLayer(group.id);
    }

    function startRename() {
        editing = true;
        requestAnimationFrame(() => editInput?.focus());
    }

    function finishRename() {
        editing = false;
        if (app.handle && editInput) {
            app.handle.set_layer_name(group.id, editInput.value);
            onupdate();
        }
    }

    function clickMaskThumb(e: MouseEvent) {
        e.stopPropagation();
        if (maskModifier !== null
            && dispatchClick('maskThumb', e, { layerId: maskModifier.id })) {
            onupdate();
            return;
        }
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

    function removeMask() {
        if (app.handle) {
            app.handle.remove_mask(group.id);
            onupdate();
        }
    }

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('text/plain', String(group.id));
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

    function onDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        const pos = dropPos;
        dropPos = 'none';
        const draggedId = e.dataTransfer?.getData('text/plain');
        if (!draggedId || !app.handle) return;
        const id = Number(draggedId);
        if (id === group.id) return;

        if (pos === 'above') {
            app.handle.move_layer(id, 'after', group.id);
        } else if (pos === 'below') {
            app.handle.move_layer(id, 'before', group.id);
        } else {
            app.handle.move_layer(id, 'into_top', group.id);
        }
        onupdate();
    }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="layer-group" style:--depth={depth}>
    <div
        class="group-header"
        class:active={isActive}
        class:drop-above={dropPos === 'above'}
        class:drop-below={dropPos === 'below'}
        class:drop-into={dropPos === 'into'}
        onclick={onLayerClick}
        ondblclick={startRename}
        role="button"
        tabindex="-1"
        draggable="true"
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
            onclick={toggleVisibility}
            onpointerdown={(e: PointerEvent) => { e.stopPropagation(); }}
            title="Toggle visibility"
        >
            <i class={group.visible ? 'fa-solid fa-eye' : 'fa-solid fa-eye-slash'}></i>
        </button>

        <button class="collapse-btn" onclick={toggleCollapsed} title="Toggle collapsed">
            <i class={group.collapsed ? 'fa-solid fa-chevron-right' : 'fa-solid fa-chevron-down'}></i>
        </button>

        <i class="folder-icon fa-solid {group.collapsed ? 'fa-folder' : 'fa-folder-open'}"></i>

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
                onclick={clickMaskThumb}
                oncontextmenu={onMaskContextMenu}
            />
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
        <button onclick={removeMask}>Delete mask</button>
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

    .folder-icon {
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
</style>
