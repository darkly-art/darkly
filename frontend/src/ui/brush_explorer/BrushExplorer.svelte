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
     * Both panes are real scrollports, so pen and touch momentum come from the
     * platform on either side. What keeps them from oscillating is that only
     * one of them is ever *driven*: `driver` names the pane the user has their
     * hand on, taken from input events, and each frame the other pane is moved
     * to match it. Nothing is derived from a `scroll` event — they only mark
     * the frame loop as live — because a programmatic `scrollTop` write lands
     * synchronously while its `scroll` event does not, and anything computed
     * from the event describes a position the pane has already left.
     */
    import Modal from '../Modal.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import type { BrushInfo } from '../../engine/protocol_gen';
    import { brushLibrary } from '../../state/brush_library.svelte';
    import { recentBrushes } from '../../state/recents.svelte';
    import { packIcon, PACK_ICON_FALLBACK } from '../../lib/packIcon';
    import BrushTile from '../brush_library/BrushTile.svelte';
    import PackWheel from './PackWheel.svelte';
    import PackProjection from './PackProjection.svelte';
    import { groupByPack, matchesQuery, packNamesByBrush, withRecents } from '../brush_library/grouping';
    import {
        FOCUS_LINE,
        present,
        scrollTopForSection,
        visibleSpan,
        type CardCurve,
        type PackBand,
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

    /** The pane the user is driving. Set from input events — a `pointerdown` or
     *  a mouse wheel is unambiguously a hand, where a `scroll` event might be
     *  the echo of our own write to the other pane. */
    let driver: 'list' | 'wheel' = 'list';

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
    /** All published together, once per frame, from one sample. Separately
     *  they are the three things that used to disagree. */
    let focused = $state<number | null>(null);
    let curves = $state<CardCurve[]>([]);
    /** One band per pack currently on screen, in explorer-local coordinates.
     *
     *  Read straight off the rendered boxes rather than derived from
     *  `geometry`: the two panes start at different heights (the search field
     *  sits above the list), the cards carry the rolodex transform, and both a
     *  section's and a card's extent are clipped by their scrollport — offsets
     *  the mapping has no reason to know about, any one of which wrong detaches
     *  a band from what it is drawn between. */
    let bands = $state<PackBand[]>([]);

    /** Both ends of a band run this far *under* what they join. The panes are
     *  positioned and so paint over the overlay, which hides the overlap
     *  entirely — and buys immunity to the half-pixel seam that subpixel layout
     *  otherwise opens between two boxes that merely abut. */
    const OVERLAP = 3;

    function updateBands() {
        if (!explorerEl || !listEl || !wheelEl) {
            bands = [];
            return;
        }
        const cards = wheelEl.querySelectorAll<HTMLElement>('.pack-card');
        const sections = sectionElements();
        const base = explorerEl.getBoundingClientRect();
        const listPort = listEl.getBoundingClientRect();
        const wheelPort = wheelEl.getBoundingClientRect();

        const next: PackBand[] = [];
        // Bounded by all three, so a group list that has changed since the last
        // measurement draws only the packs that exist in every one of them.
        const n = Math.min(cards.length, sections.length, groups.length);
        for (let i = 0; i < n; i++) {
            const s = sections[i].getBoundingClientRect();
            const arrives = visibleSpan(s.top, s.bottom, listPort.top, listPort.bottom);
            if (!arrives) continue;
            const c = cards[i].getBoundingClientRect();
            // Clipped against the wheel too: a card scrolled out of its column
            // would otherwise throw a band in from outside the pane.
            const leaves = visibleSpan(c.top, c.bottom, wheelPort.top, wheelPort.bottom);
            if (!leaves) continue;
            next.push({
                id: groups[i].id,
                primary: groups[i].primary,
                opacity: curves[i]?.opacity ?? 1,
                ribbon: {
                    x0: c.right - base.left - OVERLAP,
                    top0: leaves.top - base.top,
                    bottom0: leaves.bottom - base.top,
                    x1: s.left - base.left + OVERLAP,
                    top1: arrives.top - base.top,
                    bottom1: arrives.bottom - base.top,
                },
            });
        }
        bands = next;
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
        wake();
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

    /** How many still frames end the loop. A few, not one: native momentum can
     *  deliver two successive frames at the same position and keep going. */
    const IDLE_FRAMES = 8;
    let raf = 0;
    let idle = 0;
    let lastList = -1;
    let lastWheel = -1;

    /**
     * One frame: sample both panes, move the driven one, and publish everything
     * that depends on where they now are.
     *
     * The `scrollTop` write and the ribbon's `getBoundingClientRect` reads are
     * in the same callback on purpose — the read flushes the write and observes
     * it, so the band is drawn to where the card *is* this frame rather than
     * where it was last one.
     */
    function pump() {
        raf = 0;
        if (!listEl || !wheelEl) return;

        const frame = present(
            {
                listScrollTop: listEl.scrollTop,
                wheelScrollTop: wheelEl.scrollTop,
                driver,
            },
            geometry,
        );
        // Only ever the pane the user is *not* touching. Writing the driver's
        // own scrollTop would cancel the momentum it is running on.
        const driven = driver === 'list' ? wheelEl : listEl;
        const target = driver === 'list' ? frame.wheelScrollTop : frame.listScrollTop;
        // Sub-pixel writes are ignored: assigning a value the pane already has
        // still costs a scroll event, which would keep the loop awake forever.
        if (Math.abs(driven.scrollTop - target) > 0.5) driven.scrollTop = target;

        // Clamped to what is *rendered*. `geometry` is measured asynchronously,
        // so while a search narrows the group list its sections outlive the
        // elements by a frame, and an index past the end would leave `focused`,
        // the card it highlights and the ribbon's colour each resolving
        // differently — the exact disagreement this loop exists to prevent.
        focused = frame.focused === null ? null : Math.min(frame.focused, groups.length - 1);
        curves = frame.curves;
        updateBands();

        const moved = listEl.scrollTop !== lastList || wheelEl.scrollTop !== lastWheel;
        lastList = listEl.scrollTop;
        lastWheel = wheelEl.scrollTop;
        idle = moved ? 0 : idle + 1;
        if (idle < IDLE_FRAMES) raf = requestAnimationFrame(pump);
    }

    /** Mark the panes as moving. Every scroll event and every input lands here
     *  and nowhere else: an event's job is to keep the loop alive, never to
     *  compute anything from a position it may already have left. */
    function wake() {
        idle = 0;
        if (!raf) raf = requestAnimationFrame(pump);
    }

    function drive(side: 'list' | 'wheel') {
        driver = side;
        wake();
    }

    /** A tap on a card takes you to that pack. The list is the driver for the
     *  length of the animation — a tap is a command to move the list, and the
     *  wheel follows it frame by frame, so the card stays under the pointer and
     *  the projection stretches through the jump instead of teleporting at the
     *  end of it. */
    function jumpTo(index: number) {
        if (!listEl) return;
        drive('list');
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

    // The loop belongs to the open dialog. A closed explorer schedules nothing,
    // and the teardown is what guarantees a stray frame cannot outlive it.
    $effect(() => {
        if (!open) return;
        wake();
        return () => {
            if (raf) cancelAnimationFrame(raf);
            raf = 0;
        };
    });
</script>

<Modal bind:open title="Brushes" size="full">
    <div class="explorer" bind:this={explorerEl}>
        <PackProjection {bands} />

        <PackWheel
            {groups}
            {geometry}
            {focused}
            {curves}
            bind:el={wheelEl}
            onScroll={wake}
            onDrive={() => drive('wheel')}
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
                onscroll={wake}
                onpointerdown={() => drive('list')}
                onwheel={() => drive('list')}
            >
                {#if groups.length === 0}
                    <div class="empty">
                        {#if query}No brushes match “{query}”.{:else}No brushes yet.{/if}
                    </div>
                {:else}
                    <!-- Leading space, so the *first* pack can reach the focus
                         line. Without it the list starts already scrolled past
                         it: at scrollTop 0 the line lands inside whatever pack
                         is under it, and no amount of scrolling up can bring
                         the first one to it. -->
                    <div class="lead" style:height="{FOCUS_LINE * 100}%"></div>
                    {#each groups as group (group.id)}
                        <section
                            class="group"
                            style:--pack-primary={group.primary}
                            style:--pack-secondary={group.secondary}
                        >
                            <div class="spine" title={group.label}>
                                <Icon name={group.icon} class="spine-icon" />
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
                    <!-- Trailing space so the *last* pack can reach the focus
                         line too, instead of every final card jumping to the
                         same clamped position.
                         Deliberately sized in CSS rather than from the measured
                         viewport: the trailing space changes `scrollHeight`,
                         which is what `listScrollMax` measures, so deriving one
                         from the other is a loop that settles a full viewport
                         short. -->
                    <div class="tail" style:height="{(1 - FOCUS_LINE) * 100}%"></div>
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
    /* The strip of full-strength colour the projection lands on. It paints
     * nothing itself — the section's gradient is already at full strength here,
     * and a second painting of the same colour would show as a seam down the
     * one edge the whole design is trying to make continuous. What it
     * contributes is the width of that strip, and somewhere for the pack's icon
     * to sit. */
    .spine {
        display: flex;
        justify-content: center;
        color: var(--pack-secondary);
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
    /* Space at both ends, so the first and last packs can reach the focus line
     * like any other. The heights come from `FOCUS_LINE` itself — as
     * percentages, which the scrollport resolves against its own height with no
     * measurement involved, and which the sections are then measured relative
     * to, so the mapping picks the offset up for free. */
    .lead,
    .tail {
        flex: none;
    }
    .empty {
        font-size: 12px;
        color: var(--text-dim);
        font-style: italic;
        padding: 24px;
        text-align: center;
    }
</style>
