<script lang="ts">
    import { watchDismiss } from '../../lib/dismiss';
    import Icon from '../../icons/Icon.svelte';
    import { actions, type Action } from '../../actions/registry';
    import { registryEpoch } from '../../actions/registryEpoch.svelte';
    import { NEW_LAYER_ACTION_IDS } from '../../actions/index';

    let { onpick, onclose }: {
        onpick: (actionId: string) => void;
        onclose: () => void;
    } = $props();

    // Labels and icons come from the action registrations, so the dropdown,
    // the Layer menu and the command palette can't drift apart.
    let entries = $derived.by(() => {
        registryEpoch();
        return NEW_LAYER_ACTION_IDS
            .map(id => actions.get(id))
            .filter((a): a is Action => a !== undefined);
    });

    function onKeyDown(e: KeyboardEvent) {
        if (e.key === 'Escape') onclose();
    }

    // A pointerdown outside the menu (panel + its trigger, both tagged
    // data-keep-open="new-layer") closes it. This component only mounts while
    // open, so the listener is scoped to the open lifetime.
    $effect(() => watchDismiss('new-layer', onclose));
</script>

<svelte:window onkeydown={onKeyDown} />

<div class="new-layer-menu" data-keep-open="new-layer" role="menu">
    {#each entries as entry (entry.id)}
        <button
            class="item"
            role="menuitem"
            title={entry.description}
            onclick={() => onpick(entry.id)}
        >
            <Icon name={entry.icon} />
            <span>{entry.displayName}</span>
        </button>
    {/each}
</div>

<style>
    .new-layer-menu {
        position: absolute;
        top: 100%;
        left: 0;
        margin-top: 4px;
        z-index: 100;
        min-width: 160px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-md);
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
    }

    .item {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        padding: 6px 12px;
        background: none;
        border: none;
        color: var(--text);
        font-size: 12px;
        text-align: left;
        cursor: pointer;
        transition: background var(--transition-fast);
    }

    .item:hover {
        background: var(--bg-hover);
    }

    .item :global(svg) {
        width: 14px;
        text-align: center;
        color: var(--text-muted);
        font-size: 12px;
    }

    .item:hover :global(svg) {
        color: var(--accent);
    }
</style>
