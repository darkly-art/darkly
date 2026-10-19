<!--
  Docs search.

  Pagefind indexes `dist` after every build (see src/shared/integrations/pagefind.mjs)
  and ships a JS API alongside the index. This drives that API directly rather
  than mounting Pagefind's bundled UI, for the same reason ActionsTable is hand
  written: the bundled widget brings its own DOM and typography, and matching
  this site would mean overriding its stylesheet rule by rule.

  Two presentations, one component:
    desktop: a button in the header, Ctrl/⌘+K, opening a modal
    mobile:  the input itself, full width under the header, results in a sheet
-->
<script lang="ts">
    import { onMount } from 'svelte';
    import { isMac } from '../../lib/hotkeys';

    interface Result {
        url: string;
        title: string;
        excerpt: string;
    }

    let open = $state(false);
    let query = $state('');
    let results = $state<Result[]>([]);
    let active = $state(0);
    let searching = $state(false);
    /* Null until something has been tried; the button must not accuse the
       build of anything before anyone has asked it to search. */
    let unavailable = $state(false);
    let mac = $state(false);
    let input = $state<HTMLInputElement | null>(null);
    let dialog = $state<HTMLDialogElement | null>(null);

    let pagefind: any = null;
    let loading: Promise<any> | null = null;

    /*
     * The index does not exist until a production build has run, so this is a
     * runtime import of a path Vite must not try to resolve, hence
     * `@vite-ignore` and the absolute URL. In `astro dev` it throws, and that is
     * the expected case, not an error worth surfacing as one.
     */
    function load() {
        if (pagefind) return Promise.resolve(pagefind);
        if (!loading) {
            const url = `${window.location.origin}${import.meta.env.BASE_URL}pagefind/pagefind.js`;
            loading = import(/* @vite-ignore */ url)
                .then(async (mod) => {
                    await mod.options?.({ excerptLength: 25 });
                    pagefind = mod;
                    return mod;
                })
                .catch(() => {
                    unavailable = true;
                    return null;
                });
        }
        return loading;
    }

    async function run(q: string) {
        const trimmed = q.trim();
        if (!trimmed) {
            results = [];
            searching = false;
            return;
        }
        searching = true;
        const pf = await load();
        if (!pf) {
            searching = false;
            return;
        }
        const search = await pf.search(trimmed);
        // A slower search that resolved after a newer keystroke must not
        // overwrite the newer results.
        if (trimmed !== query.trim()) return;
        results = (await Promise.all(search.results.slice(0, 8).map((r: any) => r.data()))).map(
            (d: any) => ({
                url: d.url.replace(/\.html$/, '').replace(/\/index$/, '/'),
                title: d.meta?.title ?? d.url,
                excerpt: d.excerpt,
            }),
        );
        active = 0;
        searching = false;
    }

    $effect(() => {
        void run(query);
    });

    function show() {
        open = true;
        void load();
        queueMicrotask(() => input?.focus());
    }

    function hide() {
        open = false;
        query = '';
        results = [];
    }

    function onKeydown(event: KeyboardEvent) {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
            event.preventDefault();
            open ? hide() : show();
            return;
        }
        if (!open) return;
        if (event.key === 'Escape') {
            event.preventDefault();
            hide();
        } else if (event.key === 'ArrowDown') {
            event.preventDefault();
            active = Math.min(active + 1, results.length - 1);
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            active = Math.max(active - 1, 0);
        } else if (event.key === 'Enter' && results[active]) {
            event.preventDefault();
            window.location.href = results[active].url;
        }
    }

    onMount(() => {
        mac = isMac();
    });
</script>

<svelte:window on:keydown={onKeydown} />

<!-- Desktop: the trigger. Mobile CSS hides this in favour of the bar below. -->
<button class="search-trigger" onclick={show} aria-label="Search docs">
    <i class="fa-solid fa-magnifying-glass"></i>
    <span class="search-trigger-label">Search</span>
    <kbd>{mac ? '⌘' : 'Ctrl'}<span>K</span></kbd>
</button>

<!-- Mobile: the field itself, always visible. Tapping it opens the same sheet. -->
<button class="search-bar" onclick={show} aria-label="Search docs">
    <i class="fa-solid fa-magnifying-glass"></i>
    <span>Search the docs…</span>
</button>

{#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="search-backdrop" onclick={hide}></div>
    <div class="search-panel" role="dialog" aria-modal="true" aria-label="Search docs">
        <div class="search-field">
            <i class="fa-solid fa-magnifying-glass"></i>
            <input
                bind:this={input}
                bind:value={query}
                type="search"
                placeholder="Search the docs…"
                autocomplete="off"
                spellcheck="false"
            />
            <button class="search-close" onclick={hide} aria-label="Close search">Esc</button>
        </div>

        {#if unavailable}
            <p class="search-note">
                Search is built from the site's own pages, so it only exists in a production
                build. Run <code>make docs</code>, then <code>npm run preview</code> in
                <code>docs-site/</code>, to try it.
            </p>
        {:else if query.trim() && !results.length && !searching}
            <p class="search-note">No results for “{query.trim()}”.</p>
        {:else if results.length}
            <ul class="search-results">
                {#each results as result, i}
                    <li>
                        <a
                            href={result.url}
                            class:active={i === active}
                            onmouseenter={() => (active = i)}
                        >
                            <span class="search-result-title">{result.title}</span>
                            <span class="search-result-excerpt">{@html result.excerpt}</span>
                        </a>
                    </li>
                {/each}
            </ul>
        {:else if !query.trim()}
            <p class="search-note">Every action, tool, brush, veil and blend mode.</p>
        {/if}
    </div>
{/if}

<style>
    .search-trigger,
    .search-bar {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        background: var(--bg-raised);
        border: 1px solid var(--bg-active);
        border-radius: var(--radius-md, 6px);
        color: var(--text-muted);
        font-family: inherit;
        font-size: 0.9rem;
        cursor: pointer;
        transition: border-color 0.1s, color 0.1s;
    }

    .search-trigger:hover,
    .search-bar:hover {
        border-color: var(--text-dim);
        color: var(--text);
    }

    .search-trigger {
        padding: 0.4rem 0.6rem;
        min-width: 13rem;
    }

    .search-trigger-label {
        flex: 1;
        text-align: start;
    }

    kbd {
        display: flex;
        gap: 0.15rem;
        font-family: inherit;
        font-size: 0.75rem;
        color: var(--text-dim);
        border: 1px solid var(--bg-active);
        border-radius: var(--radius-sm, 4px);
        padding: 0.05rem 0.3rem;
    }

    /* The mobile bar is the one the requirement is about: a real, full-width
       field at the top of the page, not a magnifier tucked into a nav. */
    .search-bar {
        display: none;
        width: 100%;
        padding: 0.55rem 0.75rem;
    }

    @media (max-width: 49.999em) {
        .search-trigger {
            display: none;
        }
        .search-bar {
            display: flex;
        }
    }

    .search-backdrop {
        position: fixed;
        inset: 0;
        z-index: 40;
        background: rgb(0 0 0 / 55%);
    }

    .search-panel {
        position: fixed;
        z-index: 41;
        top: 12vh;
        left: 50%;
        transform: translateX(-50%);
        width: min(38rem, 92vw);
        max-height: 70vh;
        display: flex;
        flex-direction: column;
        background: var(--bg-raised);
        border: 1px solid var(--bg-active);
        border-radius: var(--radius-md, 6px);
        box-shadow: 0 1.5rem 3rem rgb(0 0 0 / 45%);
        overflow: hidden;
    }

    .search-field {
        display: flex;
        align-items: center;
        gap: 0.6rem;
        padding: 0.85rem 1rem;
        border-bottom: 1px solid var(--bg-active);
        color: var(--text-muted);
    }

    .search-field input {
        flex: 1;
        min-width: 0;
        background: none;
        border: 0;
        outline: none;
        color: var(--text);
        font-family: inherit;
        font-size: 1rem;
        font-weight: 300;
    }

    .search-field input::-webkit-search-cancel-button {
        display: none;
    }

    .search-close {
        background: none;
        border: 1px solid var(--bg-active);
        border-radius: var(--radius-sm, 4px);
        color: var(--text-dim);
        font-family: inherit;
        font-size: 0.75rem;
        padding: 0.1rem 0.4rem;
        cursor: pointer;
    }

    .search-close:hover {
        color: var(--text);
    }

    .search-note {
        margin: 0;
        padding: 1.25rem 1rem;
        color: var(--text-muted);
        font-size: 0.9rem;
        line-height: 1.6;
    }

    .search-note code {
        background: var(--bg-active);
        border-radius: var(--radius-sm, 4px);
        padding: 0.1em 0.4em;
        font-size: 0.85em;
    }

    .search-results {
        list-style: none;
        margin: 0;
        padding: 0.4rem;
        overflow-y: auto;
    }

    .search-results a {
        display: flex;
        flex-direction: column;
        gap: 0.2rem;
        padding: 0.6rem 0.7rem;
        border-radius: var(--radius-sm, 4px);
        color: var(--text);
        text-decoration: none;
    }

    .search-results a.active {
        background: var(--bg-hover);
    }

    .search-result-title {
        color: var(--accent-text);
        font-size: 0.95rem;
    }

    .search-result-excerpt {
        color: var(--text-muted);
        font-size: 0.85rem;
        line-height: 1.5;
        display: -webkit-box;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        -webkit-box-orient: vertical;
        overflow: hidden;
    }

    /* Pagefind wraps matched terms in <mark> inside the excerpt HTML. */
    .search-result-excerpt :global(mark) {
        background: none;
        color: var(--text);
        font-weight: 500;
    }

    @media (max-width: 49.999em) {
        .search-panel {
            top: 0;
            left: 0;
            transform: none;
            width: 100vw;
            max-height: 100dvh;
            height: 100dvh;
            border-radius: 0;
            border: 0;
        }
    }
</style>
