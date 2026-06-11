<script lang="ts">
    import { actions } from '../../actions/registry';
    import { registryEpoch } from '../../actions/registryEpoch.svelte';
    import { buildTopMenus } from './menuModel';
    import MenuItems from './MenuItems.svelte';
    import { menuBar } from '../../state/menuBar.svelte';
    import { commandPalette } from '../../state/commandPalette.svelte';
    import { config, formatHotkey } from '../../config/store.svelte';
    import { watchDismiss } from '../../lib/dismiss';

    const settingsHotkey = $derived(formatHotkey(config.get('hotkeys.openSettings') as string | undefined));
    const paletteHotkey = $derived(formatHotkey(config.get('hotkeys.commandPalette') as string | undefined));

    // Actions register asynchronously (after the editor handle boots), so the
    // menu structure is keyed on the registry epoch to recompute once it's
    // populated.
    const topMenus = $derived.by(() => {
        registryEpoch();
        return buildTopMenus(actions.all());
    });

    let openTitle = $state<string | null>(null);

    function toggle(title: string) {
        openTitle = openTitle === title ? null : title;
    }
    // Hover-switch: once a menu is open, sweeping onto another opens it without
    // a click. Hovering when nothing is open does nothing.
    function hoverEnter(title: string) {
        if (openTitle !== null) openTitle = title;
    }
    function close() {
        openTitle = null;
    }
    function onKeydown(e: KeyboardEvent) {
        if (openTitle && e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            close();
        }
    }

    // A pointerdown anywhere that isn't a keep-open menu control closes it.
    $effect(() => watchDismiss('menu', close));
</script>

<svelte:window onkeydown={onKeydown} />

<div class="menu-bar">
    <button
        class="icon-btn"
        title={settingsHotkey ? `Settings (${settingsHotkey})` : 'Settings'}
        onclick={() => actions.dispatch('openSettings', {})}
    >
        <i class="fa-solid fa-gear"></i>
    </button>
    <button
        class="icon-btn"
        title={paletteHotkey ? `Find (${paletteHotkey})` : 'Find'}
        onclick={() => (commandPalette.open = true)}
    >
        <i class="fa-solid fa-magnifying-glass"></i>
    </button>

    {#each topMenus as menu (menu.title)}
        <div class="menu-group">
            <button
                class="group-btn"
                class:active={openTitle === menu.title}
                data-keep-open="menu"
                onclick={() => toggle(menu.title)}
                onmouseenter={() => hoverEnter(menu.title)}
            >{menu.title}</button>
            {#if openTitle === menu.title}
                <div class="dropdown-surface">
                    <MenuItems entries={menu.entries} onrun={close} />
                </div>
            {/if}
        </div>
    {/each}

    <div class="spacer"></div>

    <button class="icon-btn" title="Unpin menu" onclick={() => menuBar.toggle()}>
        <i class="fa-solid fa-thumbtack"></i>
    </button>
</div>

<style>
    .menu-bar {
        display: flex;
        align-items: center;
        gap: 2px;
        height: 32px;
        padding: 0 6px;
        background: var(--bg);
        border-bottom: 1px solid var(--bg-hover);
        flex-shrink: 0;
    }

    .menu-group {
        position: relative;
    }

    .group-btn {
        padding: 5px 10px;
        background: none;
        border: none;
        border-radius: 5px;
        color: var(--text);
        font-size: 13px;
        cursor: pointer;
    }
    .group-btn:hover { background: var(--bg-hover); }
    .group-btn.active { background: var(--bg-active); }

    .dropdown-surface {
        position: absolute;
        top: 100%;
        left: 0;
        z-index: 100;
        margin-top: 4px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    }

    .spacer { flex: 1; }

    .icon-btn {
        width: 28px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: 5px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
    }
    .icon-btn:hover {
        background: var(--bg-hover);
        color: var(--text);
    }
</style>
