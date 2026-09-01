<script lang="ts">
    /**
     * The brush explorer: a near-fullscreen modal for finding a brush.
     *
     * Two scroll-synced panes. On the left a rolodex of packs, which is both a
     * minimap of where you are and a way to get somewhere: tap a card to jump,
     * or fling it once there are more packs than fit. On the right every brush
     * in the library, grouped by pack, scrolling continuously from top to
     * bottom, with a search field. The point is that finding a brush never
     * requires first finding its pack: type, or scroll, or both.
     *
     * It takes the whole screen because picking a brush closes it, so the space
     * costs nothing. It is *not* dismissed by an outside pointerdown: that is
     * what made the old dropdown impossible to use with a pen, since resting a
     * hand closed it.
     *
     * Both panes are real scrollports, so pen and touch momentum come from the
     * platform on either side. What keeps them from oscillating is that only
     * one of them is ever *driven*: `driver` names the pane the user has their
     * hand on, taken from input events, and each frame the other pane is moved
     * to match it. Nothing is derived from a `scroll` event (they only mark
     * the frame loop as live), because a programmatic `scrollTop` write lands
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
    import { packPalette } from '../../lib/packPalette';
    import BrushTile from '../brush_library/BrushTile.svelte';
    import PackWheel from './PackWheel.svelte';
    import PackProjection from './PackProjection.svelte';
    import { groupByPack, matchesQuery, packNamesByBrush, withRecents } from '../brush_library/grouping';
    import {
        FOCUS_LINE,
        PACK_RIM,
        packBands,
        present,
        sameGeometry,
        scrollTopForSection,
        type CardCurve,
        type PackBand,
        type PaneLayout,
        type SectionExtent,
        type WheelGeometry,
    } from './wheel';

    interface Props {
        open: boolean;
    }
    let { open = $bindable(false) }: Props = $props();

    /** How many recents the explorer shows. `BRUSH_CAP` in `recents.svelte.ts`
     *  stays 12; it is sized for the radial widget, and this is a view. */
    const RECENTS_SHOWN = 5;
    const RECENTS_ICON = 'fa6-solid:clock-rotate-left';
    /** Card pitch used until the wheel has two cards to measure between. */
    const FALLBACK_ADVANCE = 52;

    let query = $state('');
    let explorerEl: HTMLElement | undefined = $state();
    let listEl: HTMLElement | undefined = $state();
    let wheelEl: HTMLElement | undefined = $state();

    /** The pane the user is driving. Set from input events: a `pointerdown` or
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
     *  Computed from the frame's own sample (see `packBands`). */
    let bands = $state<PackBand[]>([]);

    /** Where the panes and the cards sit. Measured beside `geometry`, on the
     *  resize observer, because none of it moves when something scrolls. */
    let layout = $state<PaneLayout>({
        wheelTop: 0,
        wheelBottom: 0,
        listTop: 0,
        listBottom: 0,
        cardRight: 0,
        sectionLeft: 0,
        cardHeight: 0,
        cardLeft: 0,
        cardTops: [],
        width: 0,
        height: 0,
        viewportLeft: 0,
        viewportTop: 0,
    });

    /** The last geometry written, as a plain (non-reactive) local.
     *
     *  Load-bearing: `measure` runs inside an effect, so if it *read* `geometry`
     *  to decide whether to write it, the write would invalidate the read and
     *  the effect would re-run forever; Svelte kills the component with
     *  `effect_update_depth_exceeded`. Comparing against a non-reactive copy
     *  breaks that cycle, and skipping unchanged writes is what lets the
     *  measure/render loop settle.
     *
     *  Compared within a tolerance rather than exactly, because the metrics are
     *  now fractional: whole-px rounding used to absorb sub-pixel reflow chatter
     *  for free, and without a tolerance that chatter would keep the loop awake.
     *  See `sameGeometry`. */
    let lastGeometry: WheelGeometry | null = null;

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

    /** Read both scrollports, the rendered sections, and where everything sits.
     *  Reads the DOM only (never `geometry`) for the reason above.
     *
     *  This is the *only* place the DOM is measured. The frame loop reads two
     *  scroll offsets and writes one, and computes everything else from what
     *  was measured here; nothing in it asks the DOM a question, so nothing in
     *  it can force a layout or read back a value the current frame has not
     *  applied yet. */
    function measure() {
        if (!listEl || !wheelEl || !explorerEl) return;

        // Everything here is read through `getBoundingClientRect`, and nothing
        // through the `offset*` family.
        //
        // The two are not interchangeable. Layout is computed in fractional CSS
        // px; `offsetTop` and friends round that to whole ones. At 100% zoom on
        // an integer-DPR display the two agree, which is why mixing them shipped
        // looking correct. Off 100% they do not, and the differences do not stay
        // small: they used to reach the bands through a *pitch* that card `i`'s
        // position was `i` multiples of, so a third of a pixel at the top of the
        // column was two pixels by the sixth pack.
        //
        // A card's own rect is usable now only because the rolodex transform
        // sits on `.body` one level in: a transformed element reports the box
        // it is drawn as, not the box it was laid out as.
        const base = explorerEl.getBoundingClientRect();
        const wheelPort = wheelEl.getBoundingClientRect();
        const listPort = listEl.getBoundingClientRect();

        const cards = [...wheelEl.querySelectorAll<HTMLElement>('.pack-card')];
        // In the wheel's scroll-content coordinates, which is the frame
        // `wheelScrollTop` is subtracted in.
        const wheelScrolled = wheelEl.scrollTop;
        const cardTops = cards.map(
            el => el.getBoundingClientRect().top - wheelPort.top + wheelScrolled,
        );
        const first = cards[0]?.getBoundingClientRect();

        // Over the whole measured line rather than one adjacent pair: the
        // mapping wants the pitch the column actually keeps, and the endpoints
        // are a better estimator of it than any two neighbours. With fewer than
        // two cards the wheel has no scroll range, so any plausible value
        // behaves identically.
        const spanned =
            cards.length >= 2
                ? (cardTops[cards.length - 1] - cardTops[0]) / (cards.length - 1)
                : FALLBACK_ADVANCE;

        const els = sectionElements();
        const listScrolled = listEl.scrollTop;
        const sections: SectionExtent[] = els.map((el, i) => {
            const r = el.getBoundingClientRect();
            return {
                id: groups[i]?.id ?? String(i),
                top: r.top - listPort.top + listScrolled,
                height: r.height,
            };
        });

        const next: WheelGeometry = {
            cardAdvance: spanned > 0 ? spanned : FALLBACK_ADVANCE,
            wheelLead: cardTops[0] ?? 0,
            wheelViewport: wheelPort.height,
            listViewport: listPort.height,
            // `scrollHeight` has no fractional accessor, so these two keep a
            // sub-pixel residue. They bound a clamp rather than placing an edge,
            // the residue does not compound, and it never reaches a join;
            // deriving them from the section extents instead would be worse for
            // the reasons `WheelGeometry` gives.
            listScrollMax: listEl.scrollHeight - listPort.height,
            wheelScrollMax: wheelEl.scrollHeight - wheelPort.height,
            sections,
        };
        if (!sameGeometry(next, lastGeometry)) {
            lastGeometry = next;
            geometry = next;
        }

        layout = {
            wheelTop: wheelPort.top - base.top,
            wheelBottom: wheelPort.bottom - base.top,
            listTop: listPort.top - base.top,
            listBottom: listPort.bottom - base.top,
            cardRight: first ? first.right - base.left : wheelPort.right - base.left,
            sectionLeft: (els[0]?.getBoundingClientRect().left ?? listPort.left) - base.left,
            cardHeight: first?.height ?? next.cardAdvance,
            cardLeft: first ? first.left - base.left : wheelPort.left - base.left,
            cardTops,
            width: base.width,
            height: base.height,
            viewportLeft: base.left,
            viewportTop: base.top,
        };
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
        // And a card. Nothing else here resizes when only the cards do (a
        // webfont arriving late is the realistic case), and a measured line
        // notices that where a single pitch survived it.
        const card = wheelEl.querySelector('.pack-card');
        if (card) ro.observe(card);
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
     * in the same callback on purpose: the read flushes the write and observes
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
        // differently: the exact disagreement this loop exists to prevent.
        focused = frame.focused === null ? null : Math.min(frame.focused, groups.length - 1);
        curves = frame.curves;
        bands = packBands(frame, geometry, layout, groups);

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
     *  length of the animation: a tap is a command to move the list, and the
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
    {#snippet controls()}
        <input
            bind:value={query}
            type="search"
            class="search"
            placeholder="Search brushes, packs and tags…"
        />
    {/snippet}

    <div
        class="explorer"
        bind:this={explorerEl}
        style:--field-w="{layout.width}px"
        style:--field-h="{layout.height}px"
        style:--field-x="{layout.viewportLeft}px"
        style:--field-y="{layout.viewportTop}px"
        style:--pack-rim-width="{PACK_RIM}px"
        style:--section-left="{layout.sectionLeft}px"
        style:--card-right="{layout.cardRight}px"
    >
        <PackProjection {bands} />

        <PackWheel
            {groups}
            {geometry}
            paneTop={layout.wheelTop}
            paneLeft={layout.cardLeft}
            {focused}
            {curves}
            bind:el={wheelEl}
            onScroll={wake}
            onDrive={() => drive('wheel')}
            onPick={jumpTo}
        />

        <div class="list-pane">
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
                        <section class="group pack-lit pack-rim" use:packPalette={group.palette}>
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
        /* The gutter is not empty space: it is where the projection is drawn,
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
    /* Lives in the dialog's header, which was a title and a close button with a
     * whole row of nothing between them. */
    .search {
        width: 100%;
        /* Kept close to the title's own line height so moving it up here costs
         * a few pixels of header rather than a new bar's worth. */
        padding: 6px 12px;
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
     * colour, entering at the left edge (where that card is) and washing
     * right across the brushes it holds. The two panes are one object seen
     * twice, so the colour has to arrive from the side the card is on rather
     * than sit in a heading that repeats what the card already says. */
    .group {
        display: grid;
        grid-template-columns: 14px minmax(0, 1fr);
        /* Square on the left, where the projection lands: the ribbon arrives
         * flush and the field runs straight through, so a rounded corner there
         * would cut a notch out of a continuous surface. */
        border-radius: 0 10px 10px 0;

        /* What makes the join with the band resolve, and the whole of it.
         *
         * A box background is snapped to whole device pixels before it is
         * painted; a `clip-path` is rasterised against its own geometry and
         * antialiased. The band arrives here as a path, so off 100% zoom the
         * section's snapped edge and the band's unsnapped one land on the same
         * device column with different coverage: a sliver painted twice, or
         * one left bare, flipping between the two as the zoom moves the join
         * across the grid. That is the seam, and it is a disagreement about
         * *rasterisation*, not about position: the coordinate the two share is
         * already exact.
         *
         * Clipping the section puts its edge in the same regime as the band's.
         * Two antialiased edges on one coordinate carry complementary coverage,
         * which for surfaces at alpha `a` composites to `a − a²·t(1−t)`: under
         * a percent of the tint, against the half-pixel of doubling or bare
         * background that the mismatch produced.
         *
         * The card end has always resolved this way and has never shown the
         * seam, which is the evidence for this being the mechanism: a card is a
         * *transformed* element, so the compositor already antialiases its edge
         * against its geometry rather than snapping it. This gives the section
         * the same property by the means available to an untransformed box.
         *
         * The clip covers the pseudo-elements too, so the surface, the beam and
         * the rim all arrive at the join the same way. */
        clip-path: inset(0 round 0 10px 10px 0);
    }
    /* Where this section samples the field: the same one the card and the
     * ribbon paint, anchored to the viewport rather than to this box.
     *
     * `fixed` is what makes the light belong to the explorer instead of to the
     * section. The positioning area becomes the viewport, so the image does not
     * move when the list scrolls and the section slides across a stationary
     * field: the effect stated once, by the surface itself, rather than
     * maintained.
     *
     * The alternative is to counter-offset `background-position` by the scroll
     * position, which has to be published from JavaScript once a frame. The list
     * is a native scrollport, so the compositor advances it without waiting for
     * that frame: on every frame the hand is on this pane, the published offset
     * describes where the section *was*, and the light drags along with the
     * packs and snaps back when they stop. A value the compositor cannot get
     * ahead of has no such frame.
     *
     * `--field-x` / `--field-y` are the explorer's own top-left in viewport
     * coordinates, measured beside the rest of the layout, so nothing about the
     * field is on the frame loop at all. They are constant between resizes
     * because a resize is the only thing that can move the box: `Modal`'s
     * `draggable` defaults off and the explorer does not set it. A draggable
     * explorer would have to remeasure on the drag.
     *
     * No border on the leading edge, so no rim there: that edge is where the
     * projection lands, and it is interior to the pack.
     *
     * The body ramp cannot be held still the same way: there is no
     * `mask-attachment`, so a mask is always anchored to the box it masks. It
     * does not need to be: the ramp runs left to right and this pane scrolls up
     * and down, so only its x has to land, and `--section-left` puts the ramp's
     * origin on the explorer's left edge where the beam's already is. The y is
     * free, which is why it is zero rather than a number nobody would be able
     * to account for. */
    .group::before {
        --pack-field-offset: calc(-1 * var(--section-left, 0px)) 0;
        background-attachment: fixed;
        background-position: var(--field-x) var(--field-y);
        border-left-width: 0;
    }
    /* Where the projection lands. It paints nothing itself: the section's own
     * fill is already at full strength here, and a second painting of it would
     * show as a seam down the one edge the whole design is trying to make
     * continuous. What it contributes is the width of that strip, and somewhere
     * for the pack's icon to sit. */
    .spine {
        display: flex;
        justify-content: center;
    }
    /* Rides down the spine with the scroll, so a tall pack is still identified
     * when its top is far above the viewport. */
    .spine :global(.spine-icon) {
        position: sticky;
        top: 0;
        font-size: 10px;
        padding: 9px 0;
        opacity: 0.75;
        /* Chroma, as on the card the section answers to. A pack's surface is a
         * tint the theme shows through, so what the icon actually sits on is
         * the theme, and the vivid pair is the part of a palette that reads
         * against a light background and a dark one alike. */
        color: var(--pack-chroma);
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
     * like any other. The heights come from `FOCUS_LINE` itself, as
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
