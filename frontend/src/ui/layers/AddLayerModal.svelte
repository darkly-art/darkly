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
    let scrollEl = $state<HTMLElement | null>(null);
    let sectionEls: HTMLElement[] = [];

    let tabs = $derived.by(() => {
        registryEpoch();
        return buildTabs({
            sources: addSources,
            catalog: id => app.catalogs[id],
            action: id => actions.get(id),
        });
    });

    let shown = $derived(filterTabs(tabs, query));
    // One flat list behind the sections, so arrow keys and Enter run over what
    // the user actually sees rather than per-group.
    let allCards = $derived(shown.flatMap(t => t.cards));

    // A deep link names a group; jump to it once the sections exist.
    $effect(() => {
        const want = addLayerModal.tab;
        if (!want) return;
        const i = shown.findIndex(t => t.title === want);
        if (i >= 0) jumpTo(i);
    });

    // Keep the highlight in range as the query narrows the list.
    $effect(() => {
        if (selected >= allCards.length) selected = Math.max(0, allCards.length - 1);
    });

    function close() {
        addLayerModal.hide();
        if (!app.engine) return;
        // The picker adds layers by type, so the panel's view has to be re-read
        // once it closes. Mounted at the app root — reachable from the palette
        // and menu bar, not just the layer panel — so the refresh is ours to do
        // rather than the panel's.
        app.refreshLayerTree();
        app.requestFrame();
    }

    async function spawn(card: AddCard) {
        if (card.source.spawn) await card.source.spawn(card.entry);
        else actions.dispatch(card.source.action);
        open = false;
    }

    /** Scroll a group's heading to the top of the list. The rail selects a
     *  position in one list rather than swapping panes, so everything stays
     *  reachable by scrolling past it. */
    function jumpTo(i: number) {
        activeTab = i;
        // `scrollIntoView` is absent under jsdom; the state change is what the
        // component tests assert on.
        sectionEls[i]?.scrollIntoView?.({ block: 'start', behavior: 'smooth' });
    }

    /** Track which group the list is currently showing, so the rail reflects a
     *  scroll the user drove rather than only the jumps it caused. */
    function onScroll() {
        if (!scrollEl) return;
        const top = scrollEl.scrollTop;
        let current = 0;
        for (let i = 0; i < sectionEls.length; i++) {
            const el = sectionEls[i];
            if (el && el.offsetTop - scrollEl.offsetTop <= top + 8) current = i;
        }
        activeTab = current;
    }

    /** Index of a card in the flat list, for the highlight. */
    function flatIndex(tabIndex: number, cardIndex: number): number {
        let n = 0;
        for (let i = 0; i < tabIndex; i++) n += shown[i].cards.length;
        return n + cardIndex;
    }

    // Left/Right and Home/End belong to the caret while the search box has
    // focus; only Enter and the rail keys are worth intercepting there.
    function onKeyDown(e: KeyboardEvent) {
        const inSearch = e.target === searchEl;
        switch (e.key) {
            case 'Enter':
                if (allCards[selected]) {
                    e.preventDefault();
                    spawn(allCards[selected]);
                }
                break;
            case 'ArrowDown':
                e.preventDefault();
                jumpTo(Math.min(activeTab + 1, shown.length - 1));
                break;
            case 'ArrowUp':
                e.preventDefault();
                jumpTo(Math.max(activeTab - 1, 0));
                break;
            case 'ArrowRight':
                if (inSearch) break;
                e.preventDefault();
                selected = Math.min(selected + 1, allCards.length - 1);
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
    {#snippet headerControls()}
        <div class="search-wrap">
            <Icon name="fa6-solid:magnifying-glass" />
            <!-- svelte-ignore a11y_autofocus -->
            <!-- The header sits outside `.add-layer`, so the search box needs
                 the shared handler directly rather than by bubbling. -->
            <input
                bind:this={searchEl}
                type="search"
                bind:value={query}
                placeholder="Search layer types…"
                onkeydown={onKeyDown}
                autofocus
            />
        </div>
    {/snippet}

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="add-layer" role="dialog" tabindex="-1" onkeydown={onKeyDown}>
        <nav class="tab-strip">
            {#each shown as tab, i (tab.title)}
                <button
                    type="button"
                    class="tab"
                    class:active={i === activeTab}
                    onclick={() => jumpTo(i)}
                >{tab.title}</button>
            {/each}
        </nav>

        <div class="scroll" bind:this={scrollEl} onscroll={onScroll}>
            {#each shown as tab, ti (tab.title)}
                <section bind:this={sectionEls[ti]}>
                    <h3><span>{tab.title}</span></h3>
                    <div class="grid">
                        {#each tab.cards as card, ci (card.source.action + '/' + card.entry.type)}
                            {@const idx = flatIndex(ti, ci)}
                            <button
                                class="card"
                                class:selected={idx === selected}
                                title={card.entry.description ?? undefined}
                                onclick={() => spawn(card)}
                                onmouseenter={() => selected = idx}
                            >
                                <EffectPreview catalog={card.catalog} entry={card.entry} />
                                <span class="card-name">{card.entry.displayName}</span>
                            </button>
                        {/each}
                    </div>
                </section>
            {/each}
            {#if shown.length === 0}
                <div class="empty">Nothing matches “{query}”</div>
            {/if}
        </div>
    </div>
</Modal>

<style>
    .add-layer {
        display: flex;
        flex-direction: row;
        height: 100%;
        min-height: 0;
        outline: none;
    }

    .search-wrap {
        display: flex;
        align-items: center;
        gap: 8px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 6px 10px;
        color: var(--text-muted);
        font-size: 13px;
        width: min(320px, 100%);
    }
    .search-wrap:focus-within { border-color: var(--accent); }
    .search-wrap input {
        flex: 1;
        background: transparent;
        border: none;
        color: var(--text);
        font-size: 13px;
        outline: none;
        min-width: 0;
    }

    .tab-strip {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 8px 0;
        border-right: 1px solid var(--bg-hover);
        flex-shrink: 0;
        min-width: 150px;
    }
    .tab {
        background: transparent;
        border: none;
        color: var(--text-muted);
        font-size: 14px;
        font-weight: 500;
        padding: 9px 16px;
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

    .scroll {
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow-y: auto;
        padding: 4px 4px 16px 16px;
        scroll-behavior: smooth;
    }

    section + section {
        margin-top: 22px;
    }

    /* A divider rather than a title: a small caps label, then a hairline
       carrying the eye across to the edge of the grid. Sticky, so the group a
       card belongs to stays named while the list scrolls under it. */
    section h3 {
        display: flex;
        align-items: center;
        gap: 10px;
        margin: 0 0 12px;
        position: sticky;
        top: 0;
        padding: 10px 0 8px;
        z-index: 1;
        /* The list scrolls under this, so it has to be opaque — and it has to
           be the dialog's own surface (`dialog.modal` in Modal.svelte), not the
           app background the floating menus sit on. */
        background: var(--bg-active);
    }

    section h3 span {
        font-size: 12px;
        font-weight: 600;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: var(--text-muted);
        white-space: nowrap;
    }

    section h3::after {
        content: '';
        flex: 1;
        height: 1px;
        background: var(--bg-hover);
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
        gap: 12px;
    }

    .card {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 10px;
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
        font-size: 13px;
        text-align: center;
        text-transform: capitalize;
    }

    .empty {
        text-align: center;
        color: var(--text-dim);
        font-size: 13px;
        padding: 24px;
    }
</style>
