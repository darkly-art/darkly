<script lang="ts">
    import type { Subdivision, PanelType } from './tree';
    import { resolvePanel } from './panelTypes';
    import { isAnchorGroup } from './tree';
    import { workspaces, popOutSupported } from './workspaces.svelte';
    import ContextMenu, { type ContextMenuItem } from '../ContextMenu.svelte';

    let { group, workspaceId }: { group: Extract<Subdivision, { kind: 'group' }>; workspaceId: number } =
        $props();

    let activeTab = $derived(group.state.tabs[group.state.activeTabIndex] ?? group.state.tabs[0]);

    // An anchor group holds a non-movable panel (the canvas): render no tab bar,
    // so it can't be grabbed or tabbed into — only docked around (see hitTest).
    let anchor = $derived(isAnchorGroup(group.state.tabs));
    // Type-owned dispatch: the registry resolves the component; this view never
    // switches on which panel it is. Only the active tab's component is
    // rendered (mount/unmount) — load-bearing for `bindingSite` hotkey scoping,
    // so a hidden Layers panel doesn't keep the layer `Delete` scope registered.
    let ActiveComponent = $derived(activeTab ? resolvePanel(activeTab).component : null);

    function onTabPointerDown(e: PointerEvent, tab: PanelType, tabIndex: number) {
        if (e.button !== 0) return; // right-click opens the context menu
        workspaces.setActiveTab(workspaceId, group.id, tab);
        // No pointer capture: tab drag must be able to cross window boundaries,
        // so the coordinator listens at window level in every open window.
        workspaces.beginTabDrag({
            sourceWorkspaceId: workspaceId,
            groupId: group.id,
            tabType: tab,
            tabIndex,
            startX: e.clientX,
            startY: e.clientY,
        });
    }

    let menu = $state<{ x: number; y: number; tab: PanelType } | null>(null);

    function onTabContextMenu(e: MouseEvent, tab: PanelType) {
        e.preventDefault();
        menu = { x: e.clientX, y: e.clientY, tab };
    }

    let menuItems = $derived<ContextMenuItem[]>(
        menu && resolvePanel(menu.tab).poppable && popOutSupported()
            ? [{ label: 'Pop Out', onclick: () => menu && workspaces.popOut(workspaceId, group.id, menu.tab) }]
            : [{ label: 'Pop Out', disabled: true, onclick: () => {} }],
    );
</script>

<div class="panel-group">
    {#if !anchor}
        <div class="tab-bar" data-panel-tab-bar data-group-id={group.id} data-workspace-id={workspaceId}>
            {#each group.state.tabs as tab, i (tab)}
                <button
                    class="tab"
                    class:active={tab === activeTab}
                    data-tab-index={i}
                    onpointerdown={(e) => onTabPointerDown(e, tab, i)}
                    oncontextmenu={(e) => onTabContextMenu(e, tab)}
                >
                    {resolvePanel(tab).title}
                </button>
            {/each}
        </div>
    {/if}

    <div
        class="panel-body"
        data-panel-body
        data-group-id={group.id}
        data-workspace-id={workspaceId}
        data-anchor={anchor ? '' : undefined}
    >
        {#if ActiveComponent}
            <ActiveComponent />
        {/if}
    </div>
</div>

{#if menu}
    <ContextMenu x={menu.x} y={menu.y} items={menuItems} onclose={() => (menu = null)} />
{/if}

<style>
    .panel-group {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-width: 0;
        min-height: 0;
        background: var(--bg);
        overflow: hidden;
    }

    .tab-bar {
        display: flex;
        flex-shrink: 0;
        background: var(--bg-active);
        overflow: hidden;
    }

    /* Inactive tabs read clearly: recessed (darker than the bar) with a mid-tone
       label, instead of near-invisible --text-dim on the bar itself. */
    .tab {
        appearance: none;
        border: none;
        background: var(--bg-raised);
        color: var(--text-muted);
        font-size: 12px;
        padding: 6px 12px;
        cursor: pointer;
        white-space: nowrap;
        border-right: 1px solid var(--bg-hover);
        touch-action: none;
    }

    .tab:hover {
        color: var(--text);
        background: var(--bg-hover);
    }

    .tab.active {
        color: var(--text);
        background: var(--bg);
    }

    .panel-body {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }
</style>
