<script lang="ts">
    let open = $state(false);

    let currentTheme = $state(document.body.classList.contains('light') ? 'light' : 'dark');

    function toggle() {
        open = !open;
    }

    function setTheme(theme: string) {
        document.body.classList.remove('dark', 'light');
        document.body.classList.add(theme);
        currentTheme = theme;
    }

    function onWindowClick(e: MouseEvent) {
        if (open && !(e.target as HTMLElement).closest('.hamburger-container')) {
            open = false;
        }
    }
</script>

<svelte:window onclick={onWindowClick} />

<div class="hamburger-container">
    <button class="hamburger-btn" onclick={toggle} title="Menu">
        <i class="fa-solid fa-bars"></i>
    </button>

    {#if open}
        <div class="menu">
            <div class="menu-section">
                <span class="menu-label">Theme</span>
                <div class="theme-options">
                    <button
                        class="theme-btn"
                        class:active={currentTheme === 'dark'}
                        onclick={() => setTheme('dark')}
                    >Dark</button>
                    <button
                        class="theme-btn"
                        class:active={currentTheme === 'light'}
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
        min-width: 160px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 8px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
        margin-top: 4px;
    }

    .menu-section {
        padding: 4px 12px;
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
