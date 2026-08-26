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
    import { SvelteSet } from 'svelte/reactivity';
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
    import PackProjection from './PackProjection.svelte';
    import { groupByPack, matchesQuery, packNamesByBrush, withRecents } from '../brush_library/grouping';
    import {
        listToWheel,
        wheelToList,
        scrollTopForSection,
        focusedSection,
        visibleSpan,
        type Ribbon,
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
    let explorerEl: HTMLElement | undefined = $state();
    let listEl: HTMLElement | undefined = $state();
    let wheelEl: HTMLElement | undefined = $state();

    /** Group ids the user has folded shut. Session state, and deliberately not
     *  persisted: a pack you closed to get it out of the way of one search is
     *  not a pack you want closed forever. Collapsing changes the sections'
     *  heights, which the resize observer picks up, so the wheel re-maps
     *  itself with no extra plumbing. */
    let collapsed = $state(new SvelteSet<string>());

    function toggleGroup(id: string) {
        if (collapsed.has(id)) collapsed.delete(id);
        else collapsed.add(id);
    }

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
        wheelLead: 0,
        wheelViewport: 0,
        listViewport: 0,
        listScrollMax: 0,
        wheelScrollMax: 0,
        sections: [],
    });
    let focused = $state<number | null>(null);

    /** The band joining the focused card to its section, in explorer-local
     *  coordinates. Read straight off the rendered boxes on every scroll rather
     *  than derived from `geometry`: the two panes start at different heights
     *  (the search field sits above the list), the focused card carries the
     *  rolodex transform, and a section's extent is clipped by the scrollport —
     *  three offsets the mapping has no reason to know about, and any one of
     *  them wrong detaches the ribbon from what it is drawn between. */
    let ribbon = $state<Ribbon | null>(null);
    const focusedGroup = $derived(focused === null ? undefined : groups[focused]);

    function updateRibbon() {
        const section = focused === null ? undefined : sectionElements()[focused];
        const card =
            focused === null
                ? undefined
                : wheelEl?.querySelectorAll<HTMLElement>('.pack-card')[focused];
        if (!explorerEl || !listEl || !card || !section) {
            ribbon = null;
            return;
        }
        const base = explorerEl.getBoundingClientRect();
        const c = card.getBoundingClientRect();
        const s = section.getBoundingClientRect();
        const port = listEl.getBoundingClientRect();
        const span = visibleSpan(s.top, s.bottom, port.top, port.bottom);
        if (!span) {
            ribbon = null;
            return;
        }
        // Both ends run a little way *under* what they join. The panes are
        // positioned and so paint over the overlay, which hides the overlap
        // entirely — and buys immunity to the half-pixel seam that subpixel
        // layout otherwise opens between two boxes that merely abut.
        const OVERLAP = 3;
        ribbon = {
            x0: c.right - base.left - OVERLAP,
            top0: c.top - base.top,
            bottom0: c.bottom - base.top,
            x1: s.left - base.left + OVERLAP,
            top1: span.top - base.top,
            bottom1: span.bottom - base.top,
        };
    }

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
            wheelLead: cards.length > 0 ? cards[0].offsetTop : 0,
            wheelViewport: wheelEl.clientHeight,
            listViewport: listEl.clientHeight,
            listScrollMax: listEl.scrollHeight - listEl.clientHeight,
            wheelScrollMax: wheelEl.scrollHeight - wheelEl.clientHeight,
            sections,
        };
        const key = `${next.cardAdvance}|${next.wheelLead}|${next.wheelViewport}|${next.listViewport}`
            + `|${next.listScrollMax}|${next.wheelScrollMax}|`
            + sections.map(s => `${s.id}:${s.top}:${s.height}`).join(',');
        if (key !== geometryKey) {
            geometryKey = key;
            geometry = next;
        }
        focused = focusedSection(listEl.scrollTop, next);
        updateRibbon();
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
        if (sync.claim('list', now())) {
            wheelEl.scrollTop = listToWheel(listEl.scrollTop, geometry);
        }
        // Outside the claim: the ribbon is drawn from wherever the two panes
        // *are*, including the frames where this pane lost the arbitration and
        // is being driven by the other one.
        updateRibbon();
    }

    function onWheelScroll() {
        if (!listEl || !wheelEl) return;
        if (sync.claim('wheel', now())) {
            listEl.scrollTop = wheelToList(wheelEl.scrollTop, geometry);
            focused = focusedSection(listEl.scrollTop, geometry);
        }
        updateRibbon();
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
    <div class="explorer" bind:this={explorerEl}>
        <PackProjection {ribbon} primary={focusedGroup?.primary ?? 'transparent'} />

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
                    <!-- Half a viewport of leading space, so the *first* pack
                         can reach the focus line at the centre. Without it the
                         list starts already scrolled past it: at scrollTop 0
                         the centre lands half a viewport into the content,
                         which is somewhere inside the second pack, and no
                         amount of scrolling up can highlight the first. -->
                    <div class="lead"></div>
                    {#each groups as group (group.id)}
                        {@const shut = collapsed.has(group.id)}
                        <section
                            class="group"
                            class:shut
                            style:--pack-primary={group.primary}
                            style:--pack-secondary={group.secondary}
                        >
                            <button
                                type="button"
                                class="spine"
                                aria-expanded={!shut}
                                aria-label={group.label}
                                title={group.label}
                                onclick={() => toggleGroup(group.id)}
                            >
                                <Icon name={group.icon} class="spine-icon" />
                            </button>
                            {#if !shut}
                                <div class="grid">
                                    {#each group.brushes as brush (brush.id)}
                                        <BrushTile
                                            {brush}
                                            active={brush.name === brushGraph.activeBrush}
                                            onSelect={selectBrush}
                                        />
                                    {/each}
                                </div>
                            {/if}
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
        /* The gutter is not empty space — it is where the projection is drawn,
         * so it is sized to give the ribbon a turn to make rather than to
         * separate the panes. */
        column-gap: 26px;
        height: 100%;
        min-height: 0;
        /* The overlay is positioned against this box, and both panes are
         * measured in its coordinates. */
        position: relative;
    }
    .list-pane {
        display: flex;
        flex-direction: column;
        min-height: 0;
        min-width: 0;
    }
    .list-header {
        flex: none;
        /* Matches the list's own padding so the field's edges line up with the
         * sections below it. */
        padding: 0 4px 10px 0;
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
        padding: 0 4px 0 0;
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    /* A pack is named once, on its card in the wheel. Here it is only a
     * colour, entering at the left edge — where that card is — and washing
     * right across the brushes it holds. The two panes are one object seen
     * twice, so the colour has to arrive from the side the card is on rather
     * than sit in a heading that repeats what the card already says. */
    .group {
        /* Full strength at the left edge, where the ribbon lands, running out
         * across the brushes: card, ribbon and section are then one unbroken
         * colour, which only holds if this end matches what the ribbon is
         * painting rather than starting at a tint of it.
         *
         * How far it reaches is absolute, not a percentage: the pane's width
         * changes with the dialog and the column count with it, and a
         * proportional wash would colour a different number of brushes at
         * every size. */
        --fade-reach: 340px;
        display: grid;
        grid-template-columns: 14px minmax(0, 1fr);
        /* Square on the left, where the projection lands, for the same reason
         * the cards are square on the right: the two edges that face each
         * other across the gutter are the join. */
        border-radius: 0 10px 10px 0;
        color: var(--pack-secondary);
        background: linear-gradient(
            to right,
            var(--pack-primary) 0,
            var(--bg) var(--fade-reach)
        );
    }
    /* Transparent on purpose: the section's own gradient is at full strength
     * here, so a background of its own would be a second painting of the same
     * colour and any disagreement between them would show as a seam down the
     * one edge the whole design is trying to make continuous. What the spine
     * contributes is a hit area — it is the fold control, there being no
     * heading left to put one in, and the one part of a section that belongs
     * to the pack rather than to a brush. */
    .spine {
        display: flex;
        justify-content: center;
        border: none;
        background: transparent;
        color: var(--pack-secondary);
        cursor: pointer;
        padding: 0;
    }
    /* Rides down the spine with the scroll, so a tall pack is still identified
     * when its top is far above the viewport. */
    .spine :global(.spine-icon) {
        position: sticky;
        top: 0;
        font-size: 10px;
        padding: 9px 0;
        opacity: 0.75;
    }
    /* `minmax(0, …)` disables the implicit `auto` min-track-size so a wide
     * stroke preview can't push the columns past the pane. */
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
        gap: 10px;
        padding: 10px;
    }
    /* Shut, a pack is the spine alone — a bar of its colour, tall enough to
     * stay a target. The wheel says which pack it is. */
    .group.shut {
        min-height: 30px;
    }
    /* Space at both ends of the list, so every pack can reach the focus line:
     * half a viewport ahead of the first (the focus sits at the centre, so
     * that is exactly what it takes to bring the first section's top to it),
     * and a full one behind the last, which also has to clear the centre.
     * Percentages resolve against the scrollport's own definite height, so no
     * measurement is involved — and the sections are measured from the DOM
     * afterwards, so the mapping picks the offset up for free. */
    .lead {
        flex: none;
        height: 50%;
    }
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
