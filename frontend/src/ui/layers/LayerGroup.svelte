<script lang="ts">
    import { app } from '../../state/app.svelte';
    import LayerItem from './LayerItem.svelte';
    import LayerGroup from './LayerGroup.svelte';

    let { group, depth = 0, onupdate }: {
        group: {
            type: 'group'; id: number; name: string; visible: boolean;
            collapsed: boolean; passthrough: boolean; opacity: number;
            blendMode: number; children: any[];
        };
        depth?: number;
        onupdate: () => void;
    } = $props();

    let isActive = $derived(app.activeLayerId === group.id);
    let editing = $state(false);
    let editInput = $state<HTMLInputElement | null>(null);
    let dropPos = $state<'none' | 'above' | 'below' | 'into'>('none');

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
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

    function setActive() {
        app.activeLayerId = group.id;
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

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('text/plain', String(group.id));
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
        onclick={setActive}
        ondblclick={startRename}
        onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setActive(); }}}
        role="button"
        tabindex="0"
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
            title="Toggle visibility"
        >
            {group.visible ? '\u{1F441}' : '\u{2014}'}
        </button>

        <button class="collapse-btn" onclick={toggleCollapsed}>
            {group.collapsed ? '\u25B6' : '\u25BC'}
        </button>

        <button
            class="passthrough-btn"
            class:normal={!group.passthrough}
            onclick={(e: MouseEvent) => {
                e.stopPropagation();
                if (app.handle) {
                    app.handle.set_group_passthrough(group.id, !group.passthrough);
                    onupdate();
                }
            }}
            title={group.passthrough ? 'Passthrough (click for Normal)' : 'Normal (click for Passthrough)'}
        >
            {group.passthrough ? 'P' : 'N'}
        </button>

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
    </div>

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
        gap: 4px;
        padding: 4px 8px;
        cursor: pointer;
        border-left: 3px solid transparent;
        min-height: 28px;
        background: #1e1e2a;
        position: relative;
    }

    .group-header:hover {
        background: #2a2a3a;
    }

    .group-header.active {
        background: #2a2a3a;
        border-left-color: #8a6aff;
    }

    .group-header.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: #6a6aff;
        pointer-events: none;
    }

    .group-header.drop-below::after {
        content: '';
        position: absolute;
        bottom: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: #6a6aff;
        pointer-events: none;
    }

    .group-header.drop-into {
        outline: 1px solid #6a6aff;
        outline-offset: -1px;
    }

    .collapse-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        padding: 0;
        font-size: 8px;
        width: 14px;
        text-align: center;
    }

    .passthrough-btn {
        background: none;
        border: 1px solid #555;
        border-radius: 2px;
        color: #888;
        cursor: pointer;
        padding: 0 3px;
        font-size: 9px;
        font-weight: 600;
        line-height: 14px;
    }
    .passthrough-btn.normal {
        color: #8a6aff;
        border-color: #8a6aff;
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

    .group-name {
        flex: 1;
        font-size: 12px;
        color: #bba;
        font-weight: 600;
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
</style>
