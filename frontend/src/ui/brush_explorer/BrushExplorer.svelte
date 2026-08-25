<script lang="ts">
    /**
     * The brush explorer: a near-fullscreen modal for finding a brush.
     *
     * Two scroll-synced panes. On the left a rolodex of packs, which is both a
     * minimap of where you are and a way to get somewhere — tap a card to jump,
     * or fling it once there are more packs than fit. On the right every brush
     * in the library, grouped by pack, scrolling continuously from top to
     * bottom, with a search field. The point is that finding a brush never
     * requires first finding its pack: type, or scroll, or both.
     *
     * It takes the whole screen because picking a brush closes it, so the space
     * costs nothing. It is *not* dismissed by an outside pointerdown — that is
     * what made the old dropdown impossible to use with a pen, since resting a
     * hand closed it.
     *
     * The right pane is the single authority for scroll position. The wheel is
     * a projection of it that may also drive it; `ScrollSyncToken` arbitrates
     * so the two cannot oscillate. Neither position is mirrored into `$state` —
     * mirroring a scrollport into a rune and writing it back is how the
     * oscillation gets a third participant.
     */
    import Modal from '../Modal.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import type { BrushInfo } from '../../engine/protocol_gen';
    import { brushLibrary } from '../../state/brush_library.svelte';
    import { recentBrushes } from '../../state/recents.svelte';
    import { packIcon, PACK_ICON_FALLBACK } from '../../lib/packIcon';
    import { ScrollSyncToken } from '../../lib/scrollSync';
    import BrushTile from '../brush_library/BrushTile.svelte';
    import PackWheel from './PackWheel.svelte';
    import { groupByPack, matchesQuery, packNamesByBrush, withRecents } from '../brush_library/grouping';
    import {
        listToWheel,
        wheelToList,
        scrollTopForSection,
        focusedSection,
        type SectionExtent,
        type WheelGeometry,
    } from './wheel';

    interface Props {
        open: boolean;
    }
    let { open = $bindable(false) }: Props = $props();

    /** How many recents the explorer shows. `BRUSH_CAP` in `recents.svelte.ts`
     *  stays 12 — it is sized for the radial widget, and this is a view. */
    const RECENTS_SHOWN = 5;
    const RECENTS_ICON = 'fa6-solid:clock-rotate-left';
    /** Card pitch used until the wheel has two cards to measure between. */
    const FALLBACK_ADVANCE = 52;

    let query = $state('');
    let listEl: HTMLElement | undefined = $state();
    let wheelEl: HTMLElement | undefined = $state();

    const packNames = $derived(packNamesByBrush(brushLibrary.packs));
    const filtered = $derived(brushLibrary.brushes.filter(b => matchesQuery(b, query, packNames)));

    /** Groups in render order: Recents pinned on top, then packs, then the
     *  brushes no pack holds. A pack with nothing visible yields no group, so a
     *  search narrows the wheel to the packs that actually have hits. */
    const groups = $derived(
        withRecents(
            groupByPack(filtered, brushLibrary.packs, packIcon, PACK_ICON_FALLBACK),
            recentBrushes.items,
            filtered,
            RECENTS_SHOWN,
            RECENTS_ICON,
        )
    );

    /** Measured from the rendered list. A derived measurement flowing downhill
     *  from the DOM, which is the correct direction. */
    let geometry = $state<WheelGeometry>({
        cardAdvance: FALLBACK_ADVANCE,
        wheelViewport: 0,
        listViewport: 0,
        listScrollMax: 0,
        wheelScrollMax: 0,
        sections: [],
    });
    let focused = $state<number | null>(null);

    /** Identity of the last geometry written, as a plain (non-reactive) local.
     *
     *  Load-bearing: `measure` runs inside an effect, so if it *read* `geometry`
     *  to decide whether to write it, the write would invalidate the read and
     *  the effect would re-run forever — Svelte kills the component with
     *  `effect_update_depth_exceeded`. Comparing against a non-reactive key
     *  breaks that cycle, and skipping identical writes is what lets the
     *  measure/render loop settle. */
    let geometryKey = '';

    const sync = new ScrollSyncToken<'list' | 'wheel'>();
    const now = () => (typeof performance !== 'undefined' ? performance.now() : 0);

    /** Every rendered group section, in document order.
     *
     *  Queried rather than collected through `bind:this` into an array: the
     *  array is populated during render while the measuring effect reads it
     *  after, so a partially-filled array silently yields a short `sections`
     *  list and every mapped position collapses onto the last one it knows
     *  about. The DOM is the thing being measured, so ask the DOM. */
    function sectionElements(): HTMLElement[] {
        return listEl ? [...listEl.querySelectorAll<HTMLElement>(':scope > section')] : [];
    }

    /** Read both scrollports and the rendered sections. Reads the DOM only —
     *  never `geometry` — for the reason above. */
    function measure() {
        if (!listEl || !wheelEl) return;

        // The card pitch comes from the stylesheet rather than being repeated
        // here. With fewer than two cards the wheel has no scroll range, so any
        // plausible value behaves identically.
        const cards = wheelEl.querySelectorAll<HTMLElement>('.pack-card');
        const cardAdvance =
            cards.length >= 2 ? cards[1].offsetTop - cards[0].offsetTop : FALLBACK_ADVANCE;

        const els = sectionElements();
        const sections: SectionExtent[] = els.map((el, i) => ({
            id: groups[i]?.id ?? String(i),
            top: el.offsetTop,
            height: el.offsetHeight,
        }));

        const next: WheelGeometry = {
            cardAdvance: cardAdvance > 0 ? cardAdvance : FALLBACK_ADVANCE,
            wheelViewport: wheelEl.clientHeight,
            listViewport: listEl.clientHeight,
            listScrollMax: listEl.scrollHeight - listEl.clientHeight,
            wheelScrollMax: wheelEl.scrollHeight - wheelEl.clientHeight,
            sections,
        };
        const key = `${next.cardAdvance}|${next.wheelViewport}|${next.listViewport}`
            + `|${next.listScrollMax}|${next.wheelScrollMax}|`
            + sections.map(s => `${s.id}:${s.top}:${s.height}`).join(',');
        if (key !== geometryKey) {
            geometryKey = key;
            geometry = next;
        }
        focused = focusedSection(listEl.scrollTop, next);
    }

    // Re-measure whenever the group set changes or anything resizes, rather
    // than once after mount: preview strips resolve their aspect ratio a frame
    // or two late, so a one-shot measurement would bake in the wrong extents
    // until the next resize.
    $effect(() => {
        void groups;
        if (!listEl || !wheelEl) return;
        measure();
        if (typeof ResizeObserver === 'undefined') return;
        const ro = new ResizeObserver(() => measure());
        ro.observe(listEl);
        ro.observe(wheelEl);
        for (const el of sectionElements()) ro.observe(el);
        return () => ro.disconnect();
    });

    function onListScroll() {
        if (!listEl || !wheelEl) return;
        focused = focusedSection(listEl.scrollTop, geometry);
        if (!sync.claim('list', now())) return;
        wheelEl.scrollTop = listToWheel(listEl.scrollTop, geometry);
    }

    function onWheelScroll() {
        if (!listEl || !wheelEl) return;
        if (!sync.claim('wheel', now())) return;
        listEl.scrollTop = wheelToList(wheelEl.scrollTop, geometry);
        focused = focusedSection(listEl.scrollTop, geometry);
    }

    /** A tap on a card takes you to that pack. Works at every size, including
     *  when the wheel has no scroll range of its own. */
    function jumpTo(index: number) {
        if (!listEl) return;
        sync.preempt('wheel', now());
        listEl.scrollTo({ top: scrollTopForSection(index, geometry), behavior: 'smooth' });
    }

    function selectBrush(brush: BrushInfo) {
        brushGraph.loadBrush(brush.name, brush.id);
        open = false;
    }

    // A fresh query each time it opens: a search left over from last time would
    // hide most of the library at the moment you most want to see it.
    $effect(() => {
        if (open) query = '';
    });
</script>

<Modal bind:open title="Brushes" size="full">
    <div class="explorer">
        <PackWheel
            {groups}
            {geometry}
            {focused}
            bind:el={wheelEl}
            onScroll={onWheelScroll}
            onPointerDown={() => sync.preempt('wheel', now())}
            onPick={jumpTo}
        />

        <div class="list-pane">
            <div class="list-header">
                <input
                    bind:value={query}
                    type="search"
                    class="search"
                    placeholder="Search brushes, packs and tags…"
                />
            </div>

<!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="list"
                bind:this={listEl}
                onscroll={onListScroll}
                onpointerdown={() => sync.preempt('list', now())}
            >
                {#if groups.length === 0}
                    <div class="empty">
                        {#if query}No brushes match “{query}”.{:else}No brushes yet.{/if}
                    </div>
                {:else}
                    {#each groups as group (group.id)}
                        <section class="group">
                            <div class="group-header">
                                <span
                                    class="swatch"
                                    style:background={group.primary}
                                    style:border-color={group.secondary}
                                ></span>
                                <Icon name={group.icon} class="group-icon" />
                                <span class="group-label">{group.label}</span>
                            </div>
                            <div class="grid">
                                {#each group.brushes as brush (brush.id)}
                                    <BrushTile
                                        {brush}
                                        active={brush.name === brushGraph.activeBrush}
                                        onSelect={selectBrush}
                                    />
                                {/each}
                            </div>
                        </section>
                    {/each}
                    <!-- Trailing space so the *last* pack's heading can still
                         reach the top of the viewport, instead of every final
                         card jumping to the same clamped position.
                         Deliberately sized in CSS rather than from the measured
                         viewport: the trailing space changes `scrollHeight`,
                         which is what `listScrollMax` measures, so deriving one
                         from the other is a loop that settles a full viewport
                         short. -->
                    <div class="tail"></div>
                {/if}
            </div>
        </div>
    </div>
</Modal>

<style>
    /* The wheel is a fixed column; the list takes the rest. `min-height: 0` on
     * both so the panes scroll internally instead of growing the dialog. */
    .explorer {
        display: grid;
        grid-template-columns: 232px minmax(0, 1fr);
        gap: 8px;
        height: 100%;
        min-height: 0;
    }
    .list-pane {
        display: flex;
        flex-direction: column;
        min-height: 0;
        min-width: 0;
    }
    .list-header {
        flex: none;
        padding: 0 4px 10px;
    }
    .search {
        width: 100%;
        padding: 9px 12px;
        font-size: 13px;
        font-family: inherit;
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
    .list {
        flex: 1 1 auto;
        min-height: 0;
        overflow-y: auto;
        /* `measure()` reads each section's `offsetTop`, which is relative to
         * the nearest positioned ancestor. Without this the sections would be
         * measured against the dialog and every mapped position would be
         * shifted by the list's own offset. */
        position: relative;
        /* Pen and touch pan this, with momentum from the platform. */
        touch-action: pan-y;
        overscroll-behavior: contain;
        padding: 0 4px;
        display: flex;
        flex-direction: column;
        gap: 22px;
    }
    .group {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .group-header {
        display: flex;
        align-items: center;
        gap: 7px;
        position: sticky;
        top: 0;
        z-index: 1;
        padding: 6px 0;
        background: var(--bg);
    }
    .swatch {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 1.5px solid transparent;
        box-sizing: border-box;
        flex: none;
    }
    .group-header :global(.group-icon) {
        font-size: 13px;
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
    /* `minmax(0, …)` disables the implicit `auto` min-track-size so a wide
     * stroke preview can't push the columns past the pane. */
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
        gap: 10px;
    }
    /* One viewport of trailing space. `height: 100%` resolves against the
     * scrollport's own definite height, so no measurement is involved. */
    .tail {
        flex: none;
        height: 100%;
    }
    .empty {
        font-size: 12px;
        color: var(--text-dim);
        font-style: italic;
        padding: 24px;
        text-align: center;
    }
</style>
