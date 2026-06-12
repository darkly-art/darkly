<script lang="ts">
    import { actions } from '../actions/registry';
    import { registryEpoch } from '../actions/registryEpoch.svelte';
    import { buildHamburgerEntries } from './menu/menuModel';
    import MenuItems from './menu/MenuItems.svelte';
    import { menuBar } from '../state/menuBar.svelte';
    import { watchDismiss } from '../lib/dismiss';
    import Icon from '../icons/Icon.svelte';

    // Keyed on the registry epoch since actions register asynchronously, after
    // this component mounts.
    const entries = $derived.by(() => {
        registryEpoch();
        return buildHamburgerEntries(actions.all());
    });

    let open = $state(false);

    function toggle() { open = !open; }
    function close() { open = false; }

    function pin() {
        menuBar.toggle();
        close();
    }

    function onKeydown(e: KeyboardEvent) {
        if (open && e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            close();
        }
    }

    // A pointerdown anywhere that isn't a keep-open menu control closes it.
    $effect(() => watchDismiss('menu', close));
</script>

<svelte:window onkeydown={onKeydown} />

<div class="hamburger-container">
    <button class="hamburger-btn" data-keep-open="menu" onclick={toggle} title="Menu">
        <Icon name="fa6-solid:bars" />
    </button>

    {#if open}
        <div class="menu">
            <MenuItems {entries} onrun={close} />
            <div class="sep"></div>
            <button class="pin-item" data-keep-open="menu" onclick={pin}>
                <span class="icon"><Icon name="fa6-solid:thumbtack" /></span>
                <span>Pin menu to top bar</span>
            </button>
        </div>
    {/if}
</div>

<style>
    .hamburger-container {
        position: relative;
    }

    .hamburger-btn {
        width: 32px;
        height: 32px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 6px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 14px;
        transition: background 0.1s, color 0.1s;
    }

    .hamburger-btn:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .menu {
        position: absolute;
        top: 100%;
        left: 0;
        z-index: 100;
        min-width: 220px;
        /* `visible` (not auto) so submenu flyouts aren't clipped. The root
           list is short enough not to need its own scroll. */
        overflow: visible;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
        margin-top: 4px;
    }

    .pin-item {
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
    .pin-item:hover { background: var(--bg-hover); }
    .pin-item .icon {
        width: 14px;
        flex-shrink: 0;
        text-align: center;
        color: var(--text-muted);
        font-size: 12px;
    }

    .sep {
        height: 1px;
        background: var(--bg-hover);
        margin: 4px 0;
    }
</style>
