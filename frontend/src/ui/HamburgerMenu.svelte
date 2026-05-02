<script lang="ts">
    import { theme, type ThemePreference } from '../state/theme.svelte';
    import { settings } from '../state/settings.svelte';
    import { config, formatHotkey } from '../config/store.svelte';
    import { openCheatsheet } from './cheatsheet';

    let open = $state(false);

    function toggle() { open = !open; }
    function close() { open = false; }

    function setTheme(pref: ThemePreference) {
        theme.set(pref);
    }

    function openSettings() {
        settings.open = true;
        close();
    }

    function onWindowClick(e: MouseEvent) {
        if (open && !(e.target as HTMLElement).closest('.hamburger-container')) {
            open = false;
        }
    }

    const settingsHotkey = $derived(formatHotkey(config.get('hotkeys.openSettings') as string | undefined));
</script>

<svelte:window onclick={onWindowClick} />

<div class="hamburger-container">
    <button class="hamburger-btn" onclick={toggle} title="Menu">
        <i class="fa-solid fa-bars"></i>
    </button>

    {#if open}
        <div class="menu">
            <button class="menu-item" onclick={openSettings}>
                <i class="fa-solid fa-sliders"></i>
                <span>Preferences…</span>
                {#if settingsHotkey}<span class="kbd">{settingsHotkey}</span>{/if}
            </button>
            <button class="menu-item" onclick={() => { openCheatsheet(); close(); }}>
                <i class="fa-solid fa-keyboard"></i>
                <span>Hotkey Cheat Sheet</span>
            </button>
            <div class="sep"></div>
            <div class="menu-section">
                <span class="menu-label">Theme</span>
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
                    <button
                        class="theme-btn"
                        class:active={theme.preference === 'system'}
                        onclick={() => setTheme('system')}
                    >Auto</button>
                </div>
            </div>
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
        min-width: 200px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
        margin-top: 4px;
    }

    .menu-item {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        padding: 7px 14px;
        background: none;
        border: none;
        color: var(--text);
        font-size: 13px;
        text-align: left;
        cursor: pointer;
    }
    .menu-item:hover { background: var(--bg-hover); }
    .menu-item i { width: 14px; color: var(--text-muted); }
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

    .menu-section {
        padding: 4px 12px 6px;
    }

    .menu-label {
        font-size: 10px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
    }

    .theme-options {
        display: flex;
        gap: 4px;
        margin-top: 6px;
    }

    .theme-btn {
        flex: 1;
        padding: 5px 0;
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
</style>
