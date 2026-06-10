<script lang="ts">
    import type { MenuItem } from './menuModel';
    import { actions } from '../../actions/registry';
    import { config, formatHotkey } from '../../config/store.svelte';

    let { items, onrun }: { items: MenuItem[]; onrun?: () => void } = $props();

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
    function tooltipOf(item: MenuItem, enabled: boolean): string | undefined {
        if (enabled) return item.description;
        return actions.get(item.actionId)?.disabledReason?.();
    }
    function run(id: string) {
        if (!enabledOf(id)) return;
        actions.dispatch(id, {});
        onrun?.();
    }
</script>

<div class="menu-list">
    {#each items as item (item.actionId)}
        {@const enabled = enabledOf(item.actionId)}
        <button
            class="menu-item"
            disabled={!enabled}
            title={tooltipOf(item, enabled)}
            onclick={() => run(item.actionId)}
        >
            <span class="check">
                {#if checkedOf(item.actionId)}<i class="fa-solid fa-check"></i>{/if}
            </span>
            <span class="label">{item.label}</span>
            {#if hotkey(item.actionId)}<span class="kbd">{hotkey(item.actionId)}</span>{/if}
        </button>
    {/each}
</div>

<style>
    .menu-list {
        display: flex;
        flex-direction: column;
        min-width: 200px;
    }

    .menu-item {
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
    }
    .menu-item:hover:not(:disabled) { background: var(--bg-hover); }
    .menu-item:disabled {
        opacity: 0.45;
        cursor: not-allowed;
    }

    .check {
        width: 14px;
        flex-shrink: 0;
        color: var(--accent);
        font-size: 11px;
    }

    .label { flex: 1; }

    .kbd {
        margin-left: auto;
        font-family: var(--font-mono, monospace);
        font-size: 11px;
        color: var(--text-muted);
    }
</style>
