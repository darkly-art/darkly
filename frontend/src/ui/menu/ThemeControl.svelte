<script lang="ts">
    import { theme, type ThemePreference } from '../../state/theme.svelte';

    function setTheme(pref: ThemePreference) {
        theme.set(pref);
    }
</script>

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

<style>
    .menu-section {
        padding: 4px 14px 6px;
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
