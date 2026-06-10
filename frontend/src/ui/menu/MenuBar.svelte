<script lang="ts">
    import { actions } from '../../actions/registry';
    import { registryEpoch } from '../../actions/registryEpoch.svelte';
    import { buildMenu } from './menuModel';
    import MenuList from './MenuList.svelte';
    import { menuBar } from '../../state/menuBar.svelte';
    import { theme, type ThemePreference } from '../../state/theme.svelte';

    // Actions register asynchronously (after the editor handle boots), so the
    // menu structure is keyed on the registry epoch to recompute once it's
    // populated. Per-row hotkey/enabled/checked state is resolved reactively
    // inside MenuList.
    const groups = $derived.by(() => { registryEpoch(); return buildMenu(actions.all()); });

    let openGroup = $state<string | null>(null);

    function toggleGroup(title: string) {
        openGroup = openGroup === title ? null : title;
    }
    function close() {
        openGroup = null;
    }
    function onWindowClick(e: MouseEvent) {
        if (openGroup && !(e.target as HTMLElement).closest('.menu-bar')) {
            openGroup = null;
        }
    }
    function setTheme(pref: ThemePreference) {
        theme.set(pref);
    }
</script>

<svelte:window onclick={onWindowClick} />

<div class="menu-bar">
    {#each groups as group (group.title)}
        <div class="menu-group">
            <button
                class="group-btn"
                class:active={openGroup === group.title}
                onclick={() => toggleGroup(group.title)}
            >{group.title}</button>
            {#if openGroup === group.title}
                <div class="dropdown-surface">
                    <MenuList items={group.items} onrun={close} />
                </div>
            {/if}
        </div>
    {/each}

    <div class="spacer"></div>

    <div class="theme-options">
        <button
            class="theme-btn"
            class:active={theme.preference === 'dark'}
            onclick={() => setTheme('dark')}
        >Dark</button>
        <button
            class="theme-btn"
            class:active={theme.preference === 'light'}
            onclick={() => setTheme('light')}
        >Light</button>
    </div>
    <button class="pin-btn" title="Unpin menu" onclick={() => menuBar.toggle()}>
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

    .theme-options {
        display: flex;
        gap: 4px;
    }

    .theme-btn {
        padding: 4px 10px;
        background: none;
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        color: var(--text-muted);
        font-size: 12px;
        cursor: pointer;
        transition: background 0.1s, color 0.1s, border-color 0.1s;
    }
    .theme-btn:hover {
        background: var(--bg-hover);
        color: var(--text);
    }
    .theme-btn.active {
        background: var(--accent);
        border-color: var(--accent);
        color: #ffffff;
    }

    .pin-btn {
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
    .pin-btn:hover {
        background: var(--bg-hover);
        color: var(--text);
    }
</style>
