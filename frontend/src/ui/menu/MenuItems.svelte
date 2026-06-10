<script lang="ts">
    import type { MenuEntry } from './menuModel';
    import { actions } from '../../actions/registry';
    import { config, formatHotkey } from '../../config/store.svelte';
    import ThemeControl from './ThemeControl.svelte';
    import MenuItemsSelf from './MenuItems.svelte';

    let { entries, onrun }: { entries: MenuEntry[]; onrun?: () => void } = $props();

    // Which submenu flyout is currently open in THIS list. Hovering a submenu
    // row opens it; hovering any leaf row closes whatever was open — standard
    // menu behavior, so once one menu is open you can sweep across the others
    // without re-clicking.
    let openSubmenu = $state<string | null>(null);

    function hotkey(id: string): string | undefined {
        return formatHotkey(config.get('hotkeys.' + id) as string | undefined);
    }
    function enabledOf(id: string): boolean {
        const r = actions.get(id);
        return r?.enabled ? r.enabled() : true;
    }
    function checkedOf(id: string): boolean {
        const r = actions.get(id);
        return r?.checked ? r.checked() : false;
    }
    function tooltipOf(id: string, enabled: boolean): string | undefined {
        const r = actions.get(id);
        return enabled ? r?.description : r?.disabledReason?.();
    }
    function labelOf(id: string, override: string | undefined): string {
        return override ?? actions.get(id)?.displayName ?? id;
    }
    function runAction(id: string) {
        if (!enabledOf(id)) return;
        actions.dispatch(id, {});
        onrun?.();
    }
</script>

<div class="menu-items">
    {#each entries as entry, i (i)}
        {#if entry.kind === 'separator'}
            <div class="sep"></div>
        {:else if entry.kind === 'widget'}
            {#if entry.widget === 'theme'}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div onmouseenter={() => (openSubmenu = null)}>
                    <ThemeControl />
                </div>
            {/if}
        {:else if entry.kind === 'submenu'}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div
                class="row submenu-row"
                class:open={openSubmenu === entry.title}
                onmouseenter={() => (openSubmenu = entry.title)}
                onclick={() => (openSubmenu = openSubmenu === entry.title ? null : entry.title)}
            >
                <span class="icon"></span>
                <span class="label">{entry.title}</span>
                <i class="chevron fa-solid fa-chevron-right"></i>
                {#if openSubmenu === entry.title}
                    <div class="flyout">
                        <MenuItemsSelf entries={entry.entries} {onrun} />
                    </div>
                {/if}
            </div>
        {:else if actions.get(entry.actionId)}
            {@const enabled = enabledOf(entry.actionId)}
            <button
                class="row action-row"
                disabled={!enabled}
                title={tooltipOf(entry.actionId, enabled)}
                onmouseenter={() => (openSubmenu = null)}
                onclick={() => runAction(entry.actionId)}
            >
                <span class="icon">
                    {#if entry.icon}
                        <i class="fa-solid {entry.icon}"></i>
                    {:else if checkedOf(entry.actionId)}
                        <i class="fa-solid fa-check check"></i>
                    {/if}
                </span>
                <span class="label">{labelOf(entry.actionId, entry.label)}</span>
                {#if hotkey(entry.actionId)}<span class="kbd">{hotkey(entry.actionId)}</span>{/if}
            </button>
        {/if}
    {/each}
</div>

<style>
    .menu-items {
        display: flex;
        flex-direction: column;
        min-width: 200px;
    }

    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 7px 14px;
        background: none;
        border: none;
        color: var(--text);
        font-size: 13px;
        text-align: left;
        cursor: pointer;
        box-sizing: border-box;
    }
    .action-row:hover:not(:disabled),
    .submenu-row:hover,
    .submenu-row.open {
        background: var(--bg-hover);
    }
    .action-row:disabled {
        opacity: 0.45;
        cursor: not-allowed;
    }

    .icon {
        width: 14px;
        flex-shrink: 0;
        text-align: center;
        color: var(--text-muted);
        font-size: 12px;
    }
    .icon .check { color: var(--accent); font-size: 11px; }

    .label { flex: 1; }

    .chevron {
        margin-left: auto;
        font-size: 10px;
        color: var(--text-muted);
    }

    .kbd {
        margin-left: auto;
        font-family: var(--font-mono, monospace);
        font-size: 11px;
        color: var(--text-muted);
    }

    .sep {
        height: 1px;
        background: var(--bg-hover);
        margin: 4px 0;
    }

    /* Submenu flyout — opens to the right of its parent row. The hamburger
       lives at the far left, so there's always room rightward. */
    .submenu-row {
        position: relative;
    }
    .flyout {
        position: absolute;
        top: -5px;
        left: 100%;
        z-index: 10;
        margin-left: 2px;
        max-height: 80vh;
        overflow-y: auto;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    }
</style>
