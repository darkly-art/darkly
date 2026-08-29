<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { addLayerModal } from '../../state/addLayerModal.svelte';
    import { actions } from '../../actions/registry';
    import { registryEpoch } from '../../actions/registryEpoch.svelte';
    import Modal from '../Modal.svelte';
    import Icon from '../../icons/Icon.svelte';
    import EffectPreview from '../EffectPreview.svelte';
    import { addSources } from './addSources';
    import { buildTabs, filterTabs, type AddCard } from './addLayerTabs';

    // Modal owns backdrop/Escape/× dismissal and clears this when closed, which
    // we bridge back to the shared open state.
    let open = $state(true);
    $effect(() => {
        if (!open) close();
    });

    let query = $state('');
    let activeTab = $state(0);
    let selected = $state(0);
    let searchEl = $state<HTMLInputElement | null>(null);

    let tabs = $derived.by(() => {
        registryEpoch();
        return buildTabs({
            sources: addSources,
            catalog: id => app.catalogs[id],
            action: id => actions.get(id),
        });
    });

    let shown = $derived(filterTabs(tabs, query));
    let currentTab = $derived(shown[Math.min(activeTab, shown.length - 1)]);
    let cards = $derived(currentTab?.cards ?? []);

    // A deep link names a tab; land on it, else the first.
    $effect(() => {
        const want = addLayerModal.tab;
        if (!want) return;
        const i = tabs.findIndex(t => t.title === want);
        if (i >= 0) activeTab = i;
    });

    // Keep the highlight in range as the query narrows the grid.
    $effect(() => {
        if (selected >= cards.length) selected = Math.max(0, cards.length - 1);
    });

    function close() {
        addLayerModal.hide();
        if (!app.engine) return;
        // The pickers add layers and veils by type, so the panel's view of both
        // has to be re-read once one closes. Mounted at the app root — reachable
        // from the palette and menu bar, not just the layer panel — so the
        // refresh is ours to do rather than the panel's.
        app.refreshLayerTree();
        app.refreshVeilList();
        app.requestFrame();
    }

    async function spawn(card: AddCard) {
        if (card.source.spawn) await card.source.spawn(card.entry);
        else actions.dispatch(card.source.action);
        open = false;
    }

    function selectTab(i: number) {
        activeTab = i;
        selected = 0;
    }

    // Left/Right and Home/End belong to the caret while the search box has
    // focus; only Enter and the rail keys are worth intercepting there.
    function onKeyDown(e: KeyboardEvent) {
        const inSearch = e.target === searchEl;
        switch (e.key) {
            case 'Enter':
                if (cards[selected]) {
                    e.preventDefault();
                    spawn(cards[selected]);
                }
                break;
            case 'ArrowDown':
                e.preventDefault();
                selectTab(Math.min(activeTab + 1, shown.length - 1));
                break;
            case 'ArrowUp':
                e.preventDefault();
                selectTab(Math.max(activeTab - 1, 0));
                break;
            case 'ArrowRight':
                if (inSearch) break;
                e.preventDefault();
                selected = Math.min(selected + 1, cards.length - 1);
                break;
            case 'ArrowLeft':
                if (inSearch) break;
                e.preventDefault();
                selected = Math.max(selected - 1, 0);
                break;
        }
    }
</script>

<Modal bind:open title="Add Layer" size="lg">
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="add-layer" role="dialog" tabindex="-1" onkeydown={onKeyDown}>
        <div class="search-wrap">
            <Icon name="fa6-solid:magnifying-glass" />
            <!-- svelte-ignore a11y_autofocus -->
            <input
                bind:this={searchEl}
                type="search"
                bind:value={query}
                placeholder="Search layer types…"
                autofocus
            />
        </div>

        <div class="main">
            <nav class="tab-strip">
                {#each shown as tab, i (tab.title)}
                    <button
                        type="button"
                        class="tab"
                        class:active={i === Math.min(activeTab, shown.length - 1)}
                        onclick={() => selectTab(i)}
                    >{tab.title}</button>
                {/each}
            </nav>

            <div class="grid" class:single={cards.length === 1}>
                {#each cards as card, i (card.source.action + '/' + card.entry.type)}
                    <button
                        class="card"
                        class:selected={i === selected}
                        title={card.entry.description ?? undefined}
                        onclick={() => spawn(card)}
                        onmouseenter={() => selected = i}
                    >
                        <EffectPreview catalog={card.catalog} entry={card.entry} />
                        <span class="card-name">{card.entry.displayName}</span>
                    </button>
                {/each}
                {#if cards.length === 0}
                    <div class="empty">Nothing matches “{query}”</div>
                {/if}
            </div>
        </div>
    </div>
</Modal>

<style>
    .add-layer {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
        outline: none;
    }

    .search-wrap {
        display: flex;
        align-items: center;
        gap: 6px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 5px 8px;
        margin-bottom: 10px;
        color: var(--text-muted);
        font-size: 12px;
    }
    .search-wrap:focus-within { border-color: var(--accent); }
    .search-wrap input {
        flex: 1;
        background: transparent;
        border: none;
        color: var(--text);
        font-size: 12px;
        outline: none;
        min-width: 0;
    }

    .main {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: row;
    }

    .tab-strip {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 8px 0;
        border-right: 1px solid var(--bg-hover);
        flex-shrink: 0;
        min-width: 140px;
    }
    .tab {
        background: transparent;
        border: none;
        color: var(--text-muted);
        font-size: 13px;
        font-weight: 500;
        padding: 8px 16px;
        cursor: pointer;
        position: relative;
        border-radius: 0;
        text-align: left;
    }
    .tab:hover { color: var(--text); }
    .tab.active { color: var(--text); }
    .tab.active::after {
        content: '';
        position: absolute;
        top: 6px;
        bottom: 6px;
        right: -1px;
        width: 2px;
        background: var(--accent);
    }

    .grid {
        flex: 1;
        min-height: 0;
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
        grid-auto-rows: min-content;
        gap: 10px;
        padding: 8px 0 8px 16px;
        overflow-y: auto;
    }
    /* A source that is the whole choice contributes one card; let it read as a
       statement rather than a lone tile in a wide grid. */
    .grid.single {
        grid-template-columns: minmax(140px, 260px);
    }

    .card {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: var(--radius-md);
        color: var(--text);
        cursor: pointer;
        transition: background var(--transition-fast), border-color var(--transition-fast);
    }
    .card:hover,
    .card.selected {
        background: var(--bg-active);
        border-color: var(--accent);
    }

    .card-name {
        font-size: 12px;
        text-align: center;
        text-transform: capitalize;
    }

    .empty {
        grid-column: 1 / -1;
        text-align: center;
        color: var(--text-dim);
        font-size: 12px;
        padding: 20px;
    }
</style>
