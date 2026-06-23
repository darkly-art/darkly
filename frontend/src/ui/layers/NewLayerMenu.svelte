<script lang="ts">
    import { watchDismiss } from '../../lib/dismiss';
    import Icon from '../../icons/Icon.svelte';

    let { onpick, onclose }: {
        onpick: (kind: 'layer' | 'group' | 'veil' | 'void' | 'filter') => void;
        onclose: () => void;
    } = $props();

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
    <button class="item" role="menuitem" onclick={() => onpick('layer')}>
        <Icon name="fa6-solid:image" />
        <span>Normal Layer</span>
    </button>
    <button class="item" role="menuitem" onclick={() => onpick('group')}>
        <Icon name="fa6-solid:folder" />
        <span>Layer Group</span>
    </button>
    <button class="item" role="menuitem" onclick={() => onpick('veil')}>
        <Icon name="tabler:circle-half-2" />
        <span>Veil</span>
    </button>
    <button class="item" role="menuitem" onclick={() => onpick('void')}>
        <Icon name="tabler:galaxy" />
        <span>Void</span>
    </button>
    <button class="item" role="menuitem" onclick={() => onpick('filter')}>
        <Icon name="fa6-solid:circle-half-stroke" />
        <span>Filter Layer</span>
    </button>
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
