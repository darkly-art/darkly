<script lang="ts">
    import { theme, type ThemePreference } from '../state/theme.svelte';
    import { settings } from '../state/settings.svelte';
    import { config, formatHotkey } from '../config/store.svelte';
    import { openCheatsheet } from './cheatsheet';
    import { actions } from '../actions/registry';
    import { canSave } from '../storage/fileHandle';

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

    function runAction(id: string) {
        actions.dispatch(id, {});
        close();
    }

    function onWindowClick(e: MouseEvent) {
        if (open && !(e.target as HTMLElement).closest('.hamburger-container')) {
            open = false;
        }
    }

    const settingsHotkey = $derived(formatHotkey(config.get('hotkeys.openSettings') as string | undefined));
    const exportHotkey = $derived(formatHotkey(config.get('hotkeys.exportImage') as string | undefined));
    const saveHotkey = $derived(formatHotkey(config.get('hotkeys.saveDocument') as string | undefined));
    const saveAsHotkey = $derived(formatHotkey(config.get('hotkeys.saveDocumentAs') as string | undefined));
    const openHotkey = $derived(formatHotkey(config.get('hotkeys.open') as string | undefined));

    // Tooltip explaining why Save/Save As are disabled on Firefox.
    const noSaveTooltip =
        "Filesystem save isn't supported in this browser — try Chrome, Edge, or Safari.";
</script>

<svelte:window onclick={onWindowClick} />

<div class="hamburger-container">
    <button class="hamburger-btn" onclick={toggle} title="Menu">
        <i class="fa-solid fa-bars"></i>
    </button>

    {#if open}
        <div class="menu">
            <button
                class="menu-item"
                onclick={() => runAction('open')}
            >
                <i class="fa-solid fa-folder-open"></i>
                <span>Open</span>
                {#if openHotkey}<span class="kbd">{openHotkey}</span>{/if}
            </button>
            <button
                class="menu-item"
                disabled={!canSave}
                title={canSave ? undefined : noSaveTooltip}
                onclick={() => runAction('saveDocument')}
            >
                <i class="fa-solid fa-floppy-disk"></i>
                <span>Save</span>
                {#if saveHotkey}<span class="kbd">{saveHotkey}</span>{/if}
            </button>
            <button
                class="menu-item"
                disabled={!canSave}
                title={canSave ? undefined : noSaveTooltip}
                onclick={() => runAction('saveDocumentAs')}
            >
                <i class="fa-solid fa-floppy-disk"></i>
                <span>Save As</span>
                {#if saveAsHotkey}<span class="kbd">{saveAsHotkey}</span>{/if}
            </button>
            <div class="sep"></div>
            <button class="menu-item" onclick={() => runAction('exportImage')}>
                <i class="fa-solid fa-file-export"></i>
                <span>Export Image</span>
                {#if exportHotkey}<span class="kbd">{exportHotkey}</span>{/if}
            </button>
            <div class="sep"></div>
            <button class="menu-item" onclick={openSettings}>
                <i class="fa-solid fa-gear"></i>
                <span>Settings</span>
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
    .menu-item:hover:not(:disabled) { background: var(--bg-hover); }
    .menu-item:disabled {
        opacity: 0.45;
        cursor: not-allowed;
    }
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
