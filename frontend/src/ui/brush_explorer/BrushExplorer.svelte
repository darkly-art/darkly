<script lang="ts">
    /**
     * The brush explorer: a docked panel listing every brush, grouped by pack.
     *
     * Replaces a `position: fixed` dropdown that was hostile to a pen — it
     * dismissed on any outside pointerdown (so resting a hand closed it),
     * autofocused a text field, and lived inside a scrollport competing with
     * the canvas gesture guard. A panel has none of those problems: it stays
     * open while you paint, and it scrolls with the pen.
     *
     * The search query is component-local `$state`, which is also where
     * decision 7's "the query resets when the panel closes" lives: a
     * backgrounded or closed tab is unmounted by `PanelGroupView`, taking the
     * query with it. Structural, not a hook.
     */
    import { brushGraph } from '../../state/brush_graph.svelte';
    import type { BrushInfo } from '../../engine/protocol_gen';
    import { brushLibrary } from '../../state/brush_library.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { packIcon, PACK_ICON_FALLBACK } from '../../lib/packIcon';
    import BrushTile from '../brush_library/BrushTile.svelte';
    import { groupByPack, matchesQuery, packNamesByBrush } from '../brush_library/grouping';

    let query = $state('');
    let highlightIndex = $state(0);

    /** Pack names per brush, for search. */
    const packNames = $derived(packNamesByBrush(brushLibrary.packs));

    const filtered = $derived(
        brushLibrary.brushes.filter(b => matchesQuery(b, query, packNames))
    );

    /** Brushes grouped under their packs, plus a trailing "in no pack"
     *  section. See `grouping.ts`. */
    const groups = $derived(
        groupByPack(filtered, brushLibrary.packs, packIcon, PACK_ICON_FALLBACK)
    );

    /** The rendered cells, in render order.
     *
     *  Keyboard navigation indexes *this*, not `filtered`: a brush in two packs
     *  renders in two cells, so a flat index into the filter would highlight
     *  the wrong one. */
    const cells = $derived(groups.flatMap(g => g.brushes));

    // Keep the keyboard highlight in range as the filter changes.
    $effect(() => {
        const len = cells.length;
        if (highlightIndex >= len) highlightIndex = Math.max(0, len - 1);
    });

    function selectBrush(brush: BrushInfo) {
        brushGraph.loadBrush(brush.name, brush.id);
    }

    function handleKey(e: KeyboardEvent) {
        const cols = 2; // matches grid-template-columns: repeat(2, 1fr)
        const len = cells.length;
        if (len === 0) return;
        switch (e.key) {
            case 'ArrowDown':
                e.preventDefault();
                highlightIndex = Math.min(len - 1, highlightIndex + cols);
                break;
            case 'ArrowUp':
                e.preventDefault();
                highlightIndex = Math.max(0, highlightIndex - cols);
                break;
            case 'ArrowRight':
                e.preventDefault();
                highlightIndex = Math.min(len - 1, highlightIndex + 1);
                break;
            case 'ArrowLeft':
                e.preventDefault();
                highlightIndex = Math.max(0, highlightIndex - 1);
                break;
            case 'Enter':
                e.preventDefault();
                if (cells[highlightIndex]) selectBrush(cells[highlightIndex]);
                break;
        }
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="brush-explorer" onkeydown={handleKey}>
    <!-- Non-scrolling header: the search box stays put while the list below
         scrolls. Deliberately not autofocused — a panel that grabs the
         keyboard on every reveal fights a painter who reached for it with a
         pen. -->
    <div class="explorer-header">
        <input
            bind:value={query}
            type="search"
            class="search"
            placeholder="Search brushes…"
        />
    </div>

    <div class="explorer-body">
        {#if filtered.length === 0}
            <div class="empty">
                {#if query}No brushes match “{query}”.{:else}No brushes yet.{/if}
            </div>
        {:else}
            <div class="groups">
                {#each groups as group, gi (group.id)}
                    {@const offset = groups
                        .slice(0, gi)
                        .reduce((sum, g) => sum + g.brushes.length, 0)}
                    <section class="group">
                        <div class="group-header">
                            <span
                                class="pack-swatch"
                                style:background={group.primary}
                                style:border-color={group.secondary}
                            ></span>
                            <Icon name={group.icon} class="pack-icon" />
                            <span class="group-label">{group.label}</span>
                        </div>
                        <div class="grid">
                            {#each group.brushes as brush, bi (brush.id)}
                                <div
                                    class="grid-cell"
                                    class:highlight={offset + bi === highlightIndex}
                                >
                                    <BrushTile
                                        {brush}
                                        active={brush.name === brushGraph.activeBrush}
                                        onSelect={selectBrush}
                                    />
                                </div>
                            {/each}
                        </div>
                    </section>
                {/each}
            </div>
        {/if}
    </div>
</div>

<style>
    /* Fills its panel group. Non-scrolling flex column so the header stays put
     * while only `.explorer-body` scrolls. */
    .brush-explorer {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
        background: var(--bg);
    }
    .explorer-header {
        flex-shrink: 0;
        padding: 8px;
    }
    .explorer-body {
        flex: 1 1 auto;
        min-height: 0;
        overflow-y: auto;
        /* Let a pen/stylus pan the list — without this an ancestor's
         * `touch-action: none` (the canvas gesture guard) leaves it
         * unscrollable with anything but a mouse wheel. */
        touch-action: pan-y;
        /* Keep an overscroll fling from chaining out to whatever is behind
         * the panel. */
        overscroll-behavior: contain;
        padding: 0 8px 8px;
    }
    /* Raised well on the panel fill — lighter, no border. */
    .search {
        width: 100%;
        padding: 8px 10px;
        font-size: 12px;
        background: var(--bg-hover);
        color: var(--text);
        border: none;
        border-radius: var(--radius-md);
        outline: none;
        transition: background var(--transition-fast);
    }
    .search:focus {
        background: var(--bg-active);
    }
    .groups {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    .group {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .group-header {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    /* A pack's two colors, as a filled dot ringed in its secondary — enough
     * to tell packs apart at a glance without competing with the tiles. */
    .pack-swatch {
        width: 9px;
        height: 9px;
        border-radius: 50%;
        border: 1.5px solid transparent;
        box-sizing: border-box;
        flex: none;
    }
    .group-header :global(.pack-icon) {
        font-size: 12px;
        color: var(--text-muted);
        flex: none;
    }
    .group-label {
        font-size: 11px;
        font-weight: 600;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }
    .grid {
        display: grid;
        /* `minmax(0, 1fr)` disables the implicit `auto` min-track-size, so a
         * wide stroke preview can't push columns past the container. */
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }
    /* Keyboard cursor: reuse the hover fill. `.active` (loaded brush) uses a
     * lighter slab still, so the two remain distinguishable when they land on
     * the same tile. */
    .grid-cell.highlight :global(.brush-tile) {
        background: var(--bg-active);
    }
    .empty {
        font-size: 11px;
        color: var(--text-dim);
        font-style: italic;
        padding: 12px;
        text-align: center;
    }
</style>
