/**
 * How the pack wheel's position relates to the brush list's.
 *
 * Pure — no DOM, no `$state`, no reactive imports — so it is testable in
 * Vitest's node environment. Same reason `grouping.ts` sits beside its
 * component rather than inside it.
 *
 * The wheel has uniform cards; the list has sections whose heights follow their
 * contents, so a 40-brush pack is ten times taller than a 4-brush one. The
 * relation between them is therefore **piecewise linear**, with a knot at every
 * section boundary: section `i`'s list extent maps onto the wheel's slot
 * `[i·cardAdvance, (i+1)·cardAdvance)`. A uniform wheel that still points at
 * the right place.
 *
 * **Everything anchors on the focus line** — the middle of each pane,
 * {@link FOCUS_LINE}. The whole model is one sentence: *the pack across the
 * middle of the list is the pack whose card is across the middle of the wheel.*
 * Mixing anchors is how the two panes come to disagree — highlight the section
 * under one coordinate while scrolling the wheel to another and the highlighted
 * card sits somewhere the wheel never scrolled to, which is what a jump target
 * aligned to the viewport top did for as long as this file existed.
 *
 * A card therefore tracks its pack's **centre**: the pack's whole extent maps
 * onto the half-card either side of its own card, so the card is centred when
 * the pack is, and hands over to its neighbour half a card either way. That
 * bound — half a card, never more — is what keeps the highlighted card the
 * nearest one to the line at every scroll position.
 */
import type { PackPalette } from '../../lib/packPalette';

/** One group's vertical extent within the list's scroll content, measured from
 *  the rendered DOM by the component. */
export interface SectionExtent {
    /** Group id: a pack id, `''` for "in no pack", `RECENTS_ID` for recents. */
    id: string;
    /** Distance from the top of the scroll content to this section's top, px. */
    top: number;
    /** Height, px. Always > 0 — `groupByPack` drops empty groups, so a zero
     *  here is a measurement fault, and these functions clamp rather than
     *  divide by it. */
    height: number;
}

export interface WheelGeometry {
    /** Uniform per-card advance (card height + gap), px. */
    cardAdvance: number;
    /**
     * Distance from the top of the wheel's scroll content to the first card's
     * top, px — the wheel's leading pad.
     *
     * The pad is half a viewport minus half a card, which is what lets the
     * *first* and *last* cards reach the centre. Without it the wheel can only
     * centre the cards in its middle, and every mapping near an end lands on a
     * clamp instead: the stack sits against the top of the column, and the
     * focused card drifts away from the centre exactly when the list is at a
     * boundary. Measured from the DOM alongside `cardAdvance`, so the mapping
     * cannot disagree with the layout it describes.
     */
    wheelLead: number;
    /** The wheel scrollport's height, px. */
    wheelViewport: number;
    /** The list scrollport's height, px. */
    listViewport: number;
    /**
     * How far each pane can actually scroll, read from the DOM.
     *
     * **Measured, not inferred.** Deriving these from `sections` looks
     * equivalent and is not: a scrollport's real range includes padding, gaps
     * and trailing space that section extents know nothing about, and any
     * disagreement makes the mapping quietly stop moving the pane it drives
     * (`wheelMax` reading 0 pins the wheel; a short `listMax` clamps every jump
     * to the same place). If it comes from the DOM, it cannot drift from it.
     */
    listScrollMax: number;
    wheelScrollMax: number;
    /** Sections in render order. Empty when the search matches nothing. */
    sections: SectionExtent[];
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/**
 * Where the focus line sits, as a fraction of each pane's height.
 *
 * The middle, and the same for both panes, because that is the relation being
 * modelled: **the pack across the middle of the list is the pack whose card is
 * across the middle of the wheel.**
 *
 * Not a quarter of the way down, which was tried. The wheel needs the line
 * centred more than the list needs it high: a quarter leaves only a quarter of
 * the column above the line, so any pack more than a few back is scrolled off
 * the top of it, and the remaining three-quarters below sits empty. The bands
 * inherit the same skew and fan steeply downward. A rolodex wants to fan
 * symmetrically about its focus.
 */
export const FOCUS_LINE = 0.5;

/**
 * The band of colour joining the focused card to the section it points at,
 * in the coordinates of the box both panes sit in.
 *
 * The wheel compresses: a card is one uniform height whatever the size of the
 * pack behind it. The ribbon is that compression made visible — it leaves the
 * card at card height and arrives at the section at section height, so a big
 * pack fans out and a small one pinches in, and scrolling reshapes it
 * continuously as the focus moves.
 */
export interface Ribbon {
    /** The card's trailing edge, and the extent it leaves at. */
    x0: number;
    top0: number;
    bottom0: number;
    /** The section's leading edge, and the extent it arrives at. */
    x1: number;
    top1: number;
    bottom1: number;
}

/**
 * Where the panes sit and how wide a card is, in the coordinates of the box
 * that holds them both.
 *
 * Layout, not position: every field here changes only when something resizes,
 * so it is measured on the resize observer and never in the frame loop. That is
 * what lets a band be *computed* rather than read back — the alternative,
 * asking the DOM for each card's and section's rectangle every frame, both
 * forces a synchronous layout and returns the card transforms from the frame
 * before, since those are applied after the loop yields.
 */
export interface PaneLayout {
    /** The wheel scrollport's vertical extent. */
    wheelTop: number;
    wheelBottom: number;
    /** The list scrollport's vertical extent. */
    listTop: number;
    listBottom: number;
    /** The vertical line every card's trailing edge sits on. One number for
     *  every card at every scale, because the rolodex curve is anchored there
     *  (`transform-origin: right center`) — a card recedes by shrinking away
     *  from this edge, never across it. */
    cardRight: number;
    /** Where a section's leading edge is. */
    sectionLeft: number;
    /** A card's own height, which is the advance less the gap between cards. */
    cardHeight: number;
}

/**
 * One pack's band, ready to draw.
 *
 * There is one of these per pack *on screen*, not one for the focused pack. A
 * single band would have to change hands whenever the focus did, and the two
 * cards it would move between are a whole card apart at that moment — every
 * pack boundary would flick it across that gap. Drawing them all makes that
 * transition unrepresentable: a pack leaving has its band shrink to nothing as
 * its last row goes, while the next one's grows from nothing.
 */
export interface PackBand {
    /** The group's id, as a list key. */
    id: string;
    ribbon: Ribbon;
    /** The pack's palette, and the fade its own card is under — so a band sinks
     *  into the modal's black exactly as the card it leaves does. The band is
     *  the middle of the pack, so it is filled and stranded like the card and
     *  the section either side of it. */
    palette: PackPalette;
    opacity: number;
}

/**
 * The ribbon as an SVG path: two cubics with their control points on the
 * midline, which is what gives the band a settled S-curve instead of a wedge.
 *
 * Both curves share the same control abscissa, so the top and bottom edges
 * bend in step and the band keeps an even thickness through the turn.
 */
/**
 * One edge of the ribbon on its own, as an open path to be stroked.
 *
 * The band's fill and its strands are different shapes: the fill is the closed
 * ribbon, while the strands run along its top and bottom only — the two vertical
 * ends are interior to the pack, since the ribbon is the middle of a shape that
 * starts at the card and finishes at the section.
 *
 * `offset` shifts the curve down, which is how the refraction strand sits
 * directly beneath the chroma one: a stroke straddles its path, so a 2px chroma
 * stroke covers ±1px and a 1px refraction stroke centred 1.5px lower lands in
 * the 1px immediately below it.
 */
export function ribbonEdge(r: Ribbon, edge: 'top' | 'bottom', offset = 0): string {
    const mid = (r.x0 + r.x1) / 2;
    const [y0, y1] =
        edge === 'top' ? [r.top0 + offset, r.top1 + offset] : [r.bottom0 + offset, r.bottom1 + offset];
    return `M ${r.x0} ${y0} C ${mid} ${y0}, ${mid} ${y1}, ${r.x1} ${y1}`;
}

export function ribbonPath(r: Ribbon): string {
    const mid = (r.x0 + r.x1) / 2;
    return (
        `M ${r.x0} ${r.top0}` +
        ` C ${mid} ${r.top0}, ${mid} ${r.top1}, ${r.x1} ${r.top1}` +
        ` L ${r.x1} ${r.bottom1}` +
        ` C ${mid} ${r.bottom1}, ${mid} ${r.bottom0}, ${r.x0} ${r.bottom0}` +
        ` Z`
    );
}

/**
 * Trim a section's extent to what the list is actually showing.
 *
 * A pack taller than the viewport runs off both ends of it, and a ribbon drawn
 * to the untrimmed extent would fan out past the scrollport onto the search
 * field and the dialog's edge. `null` when the section is off-screen entirely,
 * which is the case where there is nothing to draw.
 */
export function visibleSpan(
    top: number,
    bottom: number,
    portTop: number,
    portBottom: number,
): { top: number; bottom: number } | null {
    const t = Math.max(top, portTop);
    const b = Math.min(bottom, portBottom);
    return b > t ? { top: t, bottom: b } : null;
}

/** The furthest either pane can be scrolled. Zero when its content fits, which
 *  is what makes a short wheel inert rather than a special case. */
export function listMax(g: WheelGeometry): number {
    return Math.max(0, g.listScrollMax);
}
export function wheelMax(g: WheelGeometry): number {
    return Math.max(0, g.wheelScrollMax);
}

/**
 * Which section contains list *content* coordinate `y`, and how far through it
 * in `[0, 1)`. The shared core of both directions.
 *
 * Clamps at both ends: a `y` above the first section reads as its start, below
 * the last as its end. `null` only when there are no sections at all.
 */
export function sectionAt(
    y: number,
    sections: SectionExtent[],
): { index: number; fraction: number } | null {
    if (sections.length === 0) return null;
    if (y < sections[0].top) return { index: 0, fraction: 0 };
    // Searched from the end for the last section that has *begun*, rather than
    // for the one containing `y`. The difference is everything the sections do
    // not cover: the list is a flex column with a gap, so between every pair is
    // a band belonging to neither, and the trailing spacer is one more. A
    // containment test matches nothing there and has to fall out of the loop
    // onto some answer — which was "the last section", throwing the wheel to
    // its far end for as long as the focus line was in a 12px gap.
    for (let i = sections.length - 1; i >= 0; i--) {
        const s = sections[i];
        if (y >= s.top) {
            // Clamped, so a gap reads as the end of the section above it and
            // the next reads as its own start — the same wheel position, which
            // is what makes crossing one cost no movement at all.
            return { index: i, fraction: s.height > 0 ? clamp((y - s.top) / s.height, 0, 1) : 0 };
        }
    }
    return { index: 0, fraction: 0 };
}

/** The list content coordinate currently on the focus line. */
function listFocus(listScrollTop: number, g: WheelGeometry): number {
    return listScrollTop + g.listViewport * FOCUS_LINE;
}

/** The wheel content coordinate currently on the focus line. */
function wheelFocus(wheelScrollTop: number, g: WheelGeometry): number {
    return wheelScrollTop + g.wheelViewport * FOCUS_LINE;
}

/** Where card `slot` sits in the wheel's scroll content. Fractional slots are
 *  meaningful: 1.5 is the point halfway between the second and third cards,
 *  which is where the wheel rests halfway between two packs. */
function cardCentre(slot: number, g: WheelGeometry): number {
    return g.wheelLead + (slot + 0.5) * g.cardAdvance;
}

/** The pads the wheel needs for its first and last cards to reach the focus
 *  line. Asymmetric, because the line is: the component applies them, and
 *  `wheelLead` is the measurement of the top one, which is what the mapping
 *  reads. */
export function wheelPadTop(g: WheelGeometry): number {
    return Math.max(0, g.wheelViewport * FOCUS_LINE - g.cardAdvance / 2);
}
export function wheelPadBottom(g: WheelGeometry): number {
    return Math.max(0, g.wheelViewport * (1 - FOCUS_LINE) - g.cardAdvance / 2);
}

/**
 * List scrollTop → the wheel scrollTop that puts the same section under the
 * wheel's centre.
 *
 * Clamped into `[0, wheelMax]`, so when the wheel's cards fit their viewport
 * this is constantly 0 and the wheel simply does not move.
 */
export function listToWheel(listScrollTop: number, g: WheelGeometry): number {
    const at = sectionAt(listFocus(listScrollTop, g), g.sections);
    if (!at) return 0;
    // `- 0.5` because a card tracks its pack's *centre*: the pack's whole
    // extent maps onto the half-card either side of its own card, so the card
    // is on the line at `fraction = 0.5` and hands over to its neighbour half a
    // card either way. Dropping the term instead ties the card to the pack's
    // start, which puts the wheel on a *boundary between* two cards whenever a
    // pack starts — the state a tap produces.
    const centre = cardCentre(at.index + at.fraction - 0.5, g);
    return clamp(centre - g.wheelViewport * FOCUS_LINE, 0, wheelMax(g));
}

/**
 * The inverse of {@link listToWheel} on the interior where neither side is
 * clamped. **Not a total inverse**: wherever either pane saturates, the round
 * trip lands at the clamp instead of where it started, because the mapping is a
 * compression (a tall section occupies one card either way).
 */
export function wheelToList(wheelScrollTop: number, g: WheelGeometry): number {
    if (g.sections.length === 0) return 0;
    // The inverse of `cardCentre(index + fraction - 0.5)`.
    const centre = wheelFocus(wheelScrollTop, g) - g.wheelLead;
    const raw = g.cardAdvance > 0 ? centre / g.cardAdvance : 0;
    const index = clamp(Math.floor(raw), 0, g.sections.length - 1);
    const fraction = clamp(raw - index, 0, 1);
    const s = g.sections[index];
    return clamp(s.top + fraction * s.height - g.listViewport * FOCUS_LINE, 0, listMax(g));
}

/**
 * The list scrollTop that a tap on card `index` commands: the section centred
 * on the focus line.
 *
 * **Centred, not aligned to the viewport top.** This was the oldest bug in the
 * explorer. Everything else anchors on the centre, and a jump that put the
 * section's top at the top of the viewport left the line half a viewport
 * lower — inside the *next* pack, for any pack shorter than the viewport. So
 * tapping a card selected its neighbour, scrolled the wheel to a card nobody
 * had touched, and drew the projection to that one. Landing the section's *top*
 * on the line instead fixes the selection but drops the pack below the middle
 * of the screen, with its predecessor filling everything above.
 *
 * A jump target has to satisfy the same anchor the highlight reads, or the two
 * disagree by construction. The end spacers are what make this reachable for
 * the first and last packs.
 */
export function scrollTopForSection(index: number, g: WheelGeometry): number {
    const s = g.sections[clamp(index, 0, g.sections.length - 1)];
    if (!s) return 0;
    return clamp(s.top + (s.height - g.listViewport) / 2, 0, listMax(g));
}

/** The section under the list viewport's centre, for highlighting its card.
 *  `null` when there are no sections. */
export function focusedSection(listScrollTop: number, g: WheelGeometry): number | null {
    return sectionAt(listFocus(listScrollTop, g), g.sections)?.index ?? null;
}

/**
 * The rolodex transform for card `index` at a given wheel position.
 *
 * Derived arithmetically rather than from a per-card `getBoundingClientRect`,
 * so styling every card costs no layout. `t` is the card centre's signed
 * distance from the scrollport centre, normalized so ±1 is the scrollport edge.
 *
 * The card at the centre is the one the list is showing, so it alone is at full
 * strength; the rest recede. `opacity` is how they recede, and against the
 * picker's black slab that reads as sinking into the background rather than
 * merely going faint — which is the point, since a column of saturated pack
 * colours at equal weight has no focus at all. The falloff is superlinear so
 * the neighbours stay legible while the far ends genuinely go dark.
 */
export interface CardCurve {
    t: number;
    rotateX: number;
    scale: number;
    opacity: number;
    /**
     * Where this card's top edge sits inside the wheel's scrollport, px.
     *
     * What lets a card's background belong to the *pane* rather than to the
     * card. Paint applied in card coordinates travels with the card, so it looks
     * identical whether the wheel is still or flying and there is nothing for the
     * eye to read as a surface catching light. Offsetting the paint by this
     * anchors it to the column instead: the light stays where it is and the
     * cards slide under it, which is the whole of the effect.
     */
    paneY: number;
}

/** A card with nothing applied to it. What a card renders as while the
 *  geometry has not caught up with a group list that just changed — flat and
 *  full strength, rather than collapsed to a scale of zero. */
export const FLAT_CURVE: CardCurve = { t: 0, rotateX: 0, scale: 1, opacity: 1, paneY: 0 };

export function cardCurve(index: number, wheelScrollTop: number, g: WheelGeometry): CardCurve {
    // Normalized against the *longer* side of the focus line, so `t` reaches
    // ±1 at the far edge of the column rather than saturating partway down it.
    const span = g.wheelViewport * Math.max(FOCUS_LINE, 1 - FOCUS_LINE);
    const t =
        span > 0 ? clamp((cardCentre(index, g) - wheelFocus(wheelScrollTop, g)) / span, -1, 1) : 0;
    const away = Math.abs(t);
    return {
        t,
        // Cards tilt away from the viewer toward the ends, the way a physical
        // rolodex reads.
        rotateX: -t * 34,
        scale: 1 - away * 0.16,
        opacity: 1 - Math.pow(away, 1.5) * 0.82,
        paneY: cardCentre(index, g) - g.cardAdvance / 2 - wheelScrollTop,
    };
}

/** Everything about the two panes at one instant, as read from the DOM. */
export interface Sample {
    listScrollTop: number;
    wheelScrollTop: number;
    /** Which pane the user is driving. Taken from input events rather than
     *  from scroll events: a scroll event cannot tell a finger from the echo
     *  of a programmatic write, and a `pointerdown` can. */
    driver: 'list' | 'wheel';
}

/** Where both panes belong, and how every card should be drawn there. */
export interface Frame {
    listScrollTop: number;
    wheelScrollTop: number;
    focused: number | null;
    curves: CardCurve[];
}

/**
 * The whole presentation of one frame, from one sample.
 *
 * This exists to make a timing bug unspeakable rather than to add behaviour:
 * every part of it was already computed by the functions above, but each was
 * called at a different moment, from a different source, on a different
 * schedule — the wheel's position from the list's `scroll` event, the card
 * transforms from the wheel's own `scroll` event a frame later, the highlight
 * from a third read. A programmatic `scrollTop` write lands synchronously while
 * the `scroll` event it provokes does not, so the transforms described where
 * the cards had been rather than where they now were, for one painted frame per
 * write. Composing the four here means they cannot be fed different numbers.
 *
 * The driven pane's position is derived; the driver's is passed through
 * untouched, so nothing ever writes back to the pane under the user's hand.
 * `curves` and `focused` are computed from the *results*, not the sample, so
 * they describe where the panes are going in this frame rather than where they
 * came from.
 */
export function present(sample: Sample, g: WheelGeometry): Frame {
    const listScrollTop =
        sample.driver === 'wheel' ? wheelToList(sample.wheelScrollTop, g) : sample.listScrollTop;
    const wheelScrollTop =
        sample.driver === 'wheel' ? sample.wheelScrollTop : listToWheel(listScrollTop, g);
    return {
        listScrollTop,
        wheelScrollTop,
        focused: focusedSection(listScrollTop, g),
        curves: g.sections.map((_, i) => cardCurve(i, wheelScrollTop, g)),
    };
}

/**
 * How far a band runs *under* the section it arrives at. The panes paint over
 * the overlay, so the overlap is invisible and buys immunity to the half-pixel
 * seam subpixel layout opens between boxes that merely abut.
 *
 * There is deliberately no counterpart at the card end. A card is tilted and
 * scaled by the rolodex curve, so it is painted as a trapezoid inside a taller
 * upright box; a band overlapping that box shows around the corners of the
 * shape actually drawn, which reads as the band sitting on top of the card
 * rather than leaving it.
 */
const SECTION_OVERLAP = 3;

/**
 * A band for every pack on screen, from the same numbers the wheel is being
 * driven with this frame.
 *
 * Computed, not measured. Both ends therefore describe where their pane will be
 * once this frame is painted, rather than where the DOM says it was before the
 * frame's `scrollTop` write — and the card end matches the card's *painted*
 * height, scaled by its own curve, instead of the upright box a tilted card
 * still reports.
 *
 * A pack contributes nothing when either end has scrolled out of its pane,
 * which is what makes the set of bands change continuously: one shrinks away as
 * its last row leaves while the next grows from nothing.
 */
export function packBands(
    frame: Frame,
    g: WheelGeometry,
    l: PaneLayout,
    packs: Array<{ id: string; palette: PackPalette }>,
): PackBand[] {
    const out: PackBand[] = [];
    const n = Math.min(packs.length, g.sections.length, frame.curves.length);
    for (let i = 0; i < n; i++) {
        const s = g.sections[i];
        const top = l.listTop + s.top - frame.listScrollTop;
        const arrives = visibleSpan(top, top + s.height, l.listTop, l.listBottom);
        if (!arrives) continue;

        // The card's *own* box, not the slot it occupies. A slot is the card
        // plus the gap to the next one, so its centre sits half a gap below the
        // card's — enough to ride visibly low against every card in the column.
        // The mapping is free to think in slots, since a uniform offset there
        // is invisible; a band drawn against a card is not.
        const scale = frame.curves[i].scale;
        const centre =
            l.wheelTop + g.wheelLead + i * g.cardAdvance + l.cardHeight / 2 - frame.wheelScrollTop;
        const half = (l.cardHeight * scale) / 2;
        const leaves = visibleSpan(centre - half, centre + half, l.wheelTop, l.wheelBottom);
        if (!leaves) continue;

        out.push({
            id: packs[i].id,
            palette: packs[i].palette,
            opacity: frame.curves[i].opacity,
            ribbon: {
                x0: l.cardRight,
                top0: leaves.top,
                bottom0: leaves.bottom,
                x1: l.sectionLeft + SECTION_OVERLAP,
                top1: arrives.top,
                bottom1: arrives.bottom,
            },
        });
    }
    return out;
}
