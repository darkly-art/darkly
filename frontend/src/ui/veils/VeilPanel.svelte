<script lang="ts">
    import { app } from '../../state/app.svelte';
    import VeilItem from './VeilItem.svelte';

    let veilTypes = $state<any[]>([]);

    function refresh() {
        app.refreshVeilList();
        app.requestFrame();
    }

    $effect(() => {
        if (app.handle) {
            try {
                veilTypes = JSON.parse(app.handle.veil_types());
            } catch { veilTypes = []; }
            refresh();
        }
    });

    function addVeil(vt: any) {
        if (!app.handle) return;
        const defaults: Record<string, any> = {};
        for (const p of vt.params) {
            defaults[p.name] = p.default;
        }
        app.handle.add_veil(vt.type, defaults);
        refresh();
    }

    function displayName(typeId: string): string {
        return typeId.replace(/_/g, ' ');
    }

    function onDragOver(e: DragEvent) {
        e.preventDefault();
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
    }

    let collapsed = $state(true);
    let showAddMenu = $state(false);

    function toggleAddMenu() {
        showAddMenu = !showAddMenu;
    }

    function handleAddVeil(vt: any) {
        addVeil(vt);
        showAddMenu = false;
    }

    function onWindowClick(e: MouseEvent) {
        if (showAddMenu) {
            const target = e.target as HTMLElement;
            if (!target.closest('.add-menu-container')) {
                showAddMenu = false;
            }
        }
    }
</script>

<svelte:window onclick={onWindowClick} />

<div class="panel" class:collapsed class:expanded={!collapsed}>
    <div class="panel-header">
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="panel-title" onclick={() => collapsed = !collapsed}>Veils</span>
        {#if !collapsed}
            <div class="add-menu-container">
                <button class="panel-btn" onclick={toggleAddMenu} title="Add veil"><i class="fa-solid fa-plus"></i></button>
                {#if showAddMenu}
                    <div class="add-menu">
                        {#each veilTypes as vt (vt.type)}
                            <button class="add-menu-item" onclick={() => handleAddVeil(vt)}>{displayName(vt.type)}</button>
                        {/each}
                    </div>
                {/if}
            </div>
        {/if}
    </div>

    {#if !collapsed}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="panel-body">
            <div class="veil-list" ondragover={onDragOver} ondrop={onDrop}>
                {#each app.veilList as veil (veil.index)}
                    <VeilItem {veil} onupdate={refresh} />
                {/each}

                {#if app.veilList.length === 0}
                    <div class="empty-message">No veils</div>
                {/if}
            </div>
        </div>
    {/if}
</div>

<style>
    .panel {
        display: flex;
        flex-direction: column;
    }

    .panel + :global(.panel) {
        border-top: 1px solid var(--bg-hover);
    }

    .panel-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 10px 12px;
        background: var(--bg-hover);
    }

    .panel-title {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
        cursor: pointer;
        transition: color 0.1s;
    }

    .panel-title:hover {
        color: var(--text);
    }

    .expanded .panel-title {
        color: var(--text);
    }

    .add-menu-container {
        position: relative;
    }

    .add-menu {
        position: absolute;
        top: 100%;
        right: 0;
        z-index: 100;
        min-width: 140px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    }

    .add-menu-item {
        display: block;
        width: 100%;
        padding: 6px 12px;
        background: none;
        border: none;
        color: var(--text);
        font-size: 12px;
        text-align: left;
        cursor: pointer;
        text-transform: capitalize;
        transition: background 0.1s;
    }

    .add-menu-item:hover {
        background: var(--bg-hover);
    }

    .panel-btn {
        width: 26px;
        height: 26px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 5px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
        transition: background 0.1s, color 0.1s;
    }

    .panel-btn:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .panel-body {
        display: flex;
        flex-direction: column;
    }

    .veil-list {
        overflow-y: auto;
        max-height: 160px;
    }

    .empty-message {
        padding: 8px;
        text-align: center;
        color: var(--text-dim);
        font-size: 12px;
    }
</style>
