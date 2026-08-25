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
 * **Everything anchors on the viewport centre.** Mapping a viewport *top*
 * coordinate while highlighting the section under the *centre* puts the
 * highlighted card half a list-viewport away from where the wheel is scrolled
 * to, which for a tall list scrolls it out of view entirely.
 */

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
    for (let i = 0; i < sections.length; i++) {
        const s = sections[i];
        // A boundary belongs to the section that starts there.
        if (y >= s.top && y < s.top + s.height) {
            return { index: i, fraction: s.height > 0 ? (y - s.top) / s.height : 0 };
        }
    }
    return { index: sections.length - 1, fraction: 1 };
}

/** The list content coordinate currently under the list viewport's centre. */
function listFocus(listScrollTop: number, g: WheelGeometry): number {
    return listScrollTop + g.listViewport / 2;
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
    const centre = (at.index + at.fraction) * g.cardAdvance;
    return clamp(centre - g.wheelViewport / 2, 0, wheelMax(g));
}

/**
 * The inverse of {@link listToWheel} on the interior where neither side is
 * clamped. **Not a total inverse**: wherever either pane saturates, the round
 * trip lands at the clamp instead of where it started, because the mapping is a
 * compression (a tall section occupies one card either way).
 */
export function wheelToList(wheelScrollTop: number, g: WheelGeometry): number {
    if (g.sections.length === 0) return 0;
    const centre = wheelScrollTop + g.wheelViewport / 2;
    const raw = g.cardAdvance > 0 ? centre / g.cardAdvance : 0;
    const index = clamp(Math.floor(raw), 0, g.sections.length - 1);
    const fraction = clamp(raw - index, 0, 1);
    const s = g.sections[index];
    const listCentre = s.top + fraction * s.height;
    return clamp(listCentre - g.listViewport / 2, 0, listMax(g));
}

/** The list scrollTop putting section `index`'s top at the top of the list
 *  viewport. What a tap on a card commands — tapping means "take me to this
 *  pack", so it aligns the heading rather than centring the section. */
export function scrollTopForSection(index: number, g: WheelGeometry): number {
    const s = g.sections[clamp(index, 0, g.sections.length - 1)];
    if (!s) return 0;
    return clamp(s.top, 0, listMax(g));
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
 */
export function cardCurve(
    index: number,
    wheelScrollTop: number,
    g: WheelGeometry,
): { t: number; rotateX: number; scale: number; opacity: number } {
    const cardCentre = (index + 0.5) * g.cardAdvance;
    const portCentre = wheelScrollTop + g.wheelViewport / 2;
    const half = g.wheelViewport / 2;
    const t = half > 0 ? clamp((cardCentre - portCentre) / half, -1, 1) : 0;
    const away = Math.abs(t);
    return {
        t,
        // Cards tilt away from the viewer toward the ends, the way a physical
        // rolodex reads.
        rotateX: -t * 34,
        scale: 1 - away * 0.16,
        opacity: 1 - away * 0.45,
    };
}
