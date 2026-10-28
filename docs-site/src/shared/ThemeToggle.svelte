<script lang="ts">
    import { onMount } from 'svelte';

    type Theme = 'dark' | 'light';
    // The contract BaseLayout's pre-paint script reads; it is stated there.
    const STORAGE_KEY = 'darkly-theme';

    let theme = $state<Theme>('dark');

    function apply(next: Theme) {
        theme = next;
        const other: Theme = next === 'dark' ? 'light' : 'dark';
        document.body.classList.remove(other);
        document.body.classList.add(next);
        document.documentElement.setAttribute('data-theme', next);
        try {
            localStorage.setItem(STORAGE_KEY, next);
        } catch {}
    }

    onMount(() => {
        const stored = (() => {
            try {
                return localStorage.getItem(STORAGE_KEY) as Theme | null;
            } catch { return null; }
        })();
        const initial: Theme = stored ?? (document.body.classList.contains('light') ? 'light' : 'dark');
        apply(initial);
    });

    function toggle() {
        apply(theme === 'dark' ? 'light' : 'dark');
    }
</script>

<button
    class="theme-toggle"
    onclick={toggle}
    title="Toggle theme"
    aria-label="Toggle theme"
>
    {#if theme === 'dark'}
        <i class="fa-solid fa-moon"></i>
    {:else}
        <i class="fa-solid fa-sun"></i>
    {/if}
</button>

<style>
    /*
     * Sized by the header that mounts it, through custom properties, so each
     * header shrinks it at its own breakpoints. The fallbacks are the full
     * size.
     */
    .theme-toggle {
        width: var(--theme-toggle-size, 32px);
        height: var(--theme-toggle-size, 32px);
        display: inline-flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: 1px solid var(--bg-active);
        border-radius: var(--radius-md, 6px);
        color: var(--text-muted);
        cursor: pointer;
        font-size: var(--theme-toggle-font, 14px);
        transition: background 0.1s, color 0.1s, border-color 0.1s;
    }
    .theme-toggle:hover {
        background: var(--bg-hover);
        color: var(--text);
        border-color: var(--text-muted);
    }
</style>
