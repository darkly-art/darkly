import { describe, it, expect } from 'vitest';
import {
    sectionAt,
    listToWheel,
    wheelToList,
    scrollTopForSection,
    focusedSection,
    cardCurve,
    cardEdge,
    listMax,
    wheelMax,
    wheelPadTop,
    wheelPadBottom,
    ribbonCorePath,
    ribbonPath,
    ribbonRimPath,
    sameGeometry,
    visibleSpan,
    present,
    packBands,
    FOCUS_LINE,
    type WheelGeometry,
    type SectionExtent,
    type PaneLayout,
} from '../wheel';

/** Sections of deliberately uneven height, which is the whole point of a
 *  piecewise map: 100 / 400 / 100 against a uniform 60px card. */
const SECTIONS: SectionExtent[] = [
    { id: 'a', top: 0, height: 100 },
    { id: 'b', top: 100, height: 400 },
    { id: 'c', top: 500, height: 100 },
];

/** Both panes scrollable, and *unpadded*: the shape the panes have before the
 *  component gives them room for their end items to reach the focus line. Used
 *  for the mapping's own arithmetic and for the clamped cases. */
const G: WheelGeometry = {
    cardAdvance: 60,
    wheelLead: 0,
    wheelViewport: 120,
    listViewport: 200,
    listScrollMax: 400,
    wheelScrollMax: 60,
    sections: SECTIONS,
};

/** A wheel whose cards fit, so it has no scroll range of its own. */
const SHORT: WheelGeometry = { ...G, wheelViewport: 400, wheelScrollMax: 0 };

const EMPTY: WheelGeometry = { ...G, sections: [], listScrollMax: 0, wheelScrollMax: 0 };

/**
 * Both panes spaced the way the component builds them, for a focus line down
 * the middle of each.
 *
 * List: 100 of lead, three sections, 100 of tail in a 200px port: 800 of
 * content, 600 of range. Wheel: 70 of pad either side of three 60px cards in a
 * 200px port: 320 of content, 120 of range. Sized so no clamp binds at either
 * end, which is what lets the tests below assert exact positions.
 */
const SPACED: WheelGeometry = {
    cardAdvance: 60,
    wheelLead: 70,
    wheelViewport: 200,
    listViewport: 200,
    listScrollMax: 600,
    wheelScrollMax: 120,
    sections: [
        { id: 'a', top: 100, height: 100 },
        { id: 'b', top: 200, height: 400 },
        { id: 'c', top: 600, height: 100 },
    ],
};

/** Where the list must be scrolled for content coordinate `y` to sit on the
 *  focus line. The tests state their intent in content coordinates; this is the
 *  one conversion they need. */
const toLine = (y: number, g: WheelGeometry) => y - g.listViewport * FOCUS_LINE;

/** How far card `i` is from the focus line, in cards. Zero means "on it". */
const offLine = (i: number, wheelScrollTop: number, g: WheelGeometry) =>
    (cardCurve(i, wheelScrollTop, g).t *
        g.wheelViewport *
        Math.max(FOCUS_LINE, 1 - FOCUS_LINE)) /
    g.cardAdvance;

/** The wheel position the list is at `listScrollTop` implies. */
const wheelFor = (listScrollTop: number, g: WheelGeometry) => listToWheel(listScrollTop, g);

describe('sectionAt', () => {
    it('a boundary belongs to the section that starts there', () => {
        expect(sectionAt(100, SECTIONS)).toEqual({ index: 1, fraction: 0 });
        expect(sectionAt(500, SECTIONS)).toEqual({ index: 2, fraction: 0 });
    });

    it('clamps past either end rather than returning null', () => {
        expect(sectionAt(-50, SECTIONS)).toEqual({ index: 0, fraction: 0 });
        expect(sectionAt(9999, SECTIONS)).toEqual({ index: 2, fraction: 1 });
    });

    it('is null only when there are no sections', () => {
        expect(sectionAt(0, [])).toBeNull();
    });

    it('resolves the gap between two sections to the one above it', () => {
        // The sections do not tile the list: `.list` is a flex column with a
        // 12px gap, so between every pair is a band of content belonging to
        // neither. A coordinate there used to match nothing and fall out of the
        // loop onto its "past the end" answer (the *last* section), so a focus
        // line crossing any gap threw the wheel to its maximum for one frame
        // and the card stack flew upward. An easing animation samples ever
        // closer together as it settles, which is why it flashed just before
        // the end and only most of the time.
        const gapped: SectionExtent[] = [
            { id: 'a', top: 0, height: 100 },
            { id: 'b', top: 112, height: 400 },
            { id: 'c', top: 524, height: 100 },
        ];
        for (let y = 100; y < 112; y++) {
            expect(sectionAt(y, gapped)).toEqual({ index: 0, fraction: 1 });
        }
        expect(sectionAt(518, gapped)).toEqual({ index: 1, fraction: 1 });
    });

    it('is continuous across a gap', () => {
        // Leaving section `i` at fraction 1 and entering `i + 1` at fraction 0
        // are the same wheel position, so crossing a gap moves the wheel by
        // nothing at all.
        const gapped: SectionExtent[] = [
            { id: 'a', top: 0, height: 100 },
            { id: 'b', top: 112, height: 400 },
        ];
        const g = { ...SPACED, sections: gapped, listViewport: 0 };
        expect(listToWheel(100, g)).toBeCloseTo(listToWheel(112, g), 5);
    });

    it('reports how far through a section it is', () => {
        expect(sectionAt(300, SECTIONS)).toEqual({ index: 1, fraction: 0.5 });
    });
});

describe('content and scroll extents', () => {
    it('reports the measured range of each pane', () => {
        expect(listMax(G)).toBe(400);
        expect(wheelMax(G)).toBe(60);
    });

    it('a pane whose content fits has no scroll range', () => {
        expect(wheelMax(SHORT)).toBe(0);
        expect(listMax(EMPTY)).toBe(0);
    });

    it('never reports a negative range', () => {
        // A scrollport measured mid-layout can report a content box smaller
        // than its client box; the mapping must clamp rather than invert.
        expect(listMax({ ...G, listScrollMax: -30 })).toBe(0);
        expect(wheelMax({ ...G, wheelScrollMax: -30 })).toBe(0);
    });
});

describe('the focus line', () => {
    it('is where a pack becomes the one you are looking at', () => {
        // Not the viewport top: the pack across the *middle* is the focused
        // one, which is a different pack from the one the list starts with.
        expect(focusedSection(0, G)).toBe(1);
        expect(focusedSection(toLine(SECTIONS[2].top + 50, G), G)).toBe(2);
    });

    it('centres a card exactly when its pack is centred', () => {
        // The invariant the two panes exist to maintain, and the one the user
        // stated: the pack across the middle of the list is the pack whose card
        // is across the middle of the wheel. A card therefore tracks its pack's
        // *centre*, not its start: the whole pack's extent maps across the
        // half-card either side of its card.
        for (let i = 0; i < SPACED.sections.length; i++) {
            const s = SPACED.sections[i];
            const listScrollTop = toLine(s.top + s.height / 2, SPACED);
            expect(focusedSection(listScrollTop, SPACED)).toBe(i);
            expect(offLine(i, wheelFor(listScrollTop, SPACED), SPACED)).toBeCloseTo(0, 5);
        }
    });

    it('never lets the focused card drift more than one card off it', () => {
        // Between two pack starts the wheel glides from one card to the next,
        // so the focused card is off the line by at most the card it is being
        // replaced by.
        for (let y = 0; y <= listMax(SPACED); y += 7) {
            const focused = focusedSection(y, SPACED)!;
            expect(Math.abs(offLine(focused, listToWheel(y, SPACED), SPACED))).toBeLessThanOrEqual(
                1.0001,
            );
        }
    });

    it('never lets the focused card drift more than half a card off it', () => {
        // A pack's whole extent maps onto the half-card either side of its own
        // card, so the focused card is never further than that from the line,
        // and is therefore always the *nearest* card to it. There is no scroll
        // position at which the highlight is somewhere the eye is not.
        for (let y = 0; y <= listMax(SPACED); y += 7) {
            const wheel = wheelFor(y, SPACED);
            const mine = Math.abs(offLine(focusedSection(y, SPACED)!, wheel, SPACED));
            expect(mine).toBeLessThanOrEqual(0.5001);
            SPACED.sections.forEach((_, i) => {
                expect(mine).toBeLessThanOrEqual(Math.abs(offLine(i, wheel, SPACED)) + 1e-9);
            });
        }
    });
});

describe('listToWheel', () => {
    it('maps uneven sections onto uniform cards', () => {
        // A 400px section and a 100px one both map onto one 60px card: at the
        // middle of the tall one its card is on the line, with its neighbours
        // exactly a card either side.
        const wheel = wheelFor(toLine(SPACED.sections[1].top + 200, SPACED), SPACED);
        expect(offLine(0, wheel, SPACED)).toBeCloseTo(-1, 5);
        expect(offLine(1, wheel, SPACED)).toBeCloseTo(0, 5);
        expect(offLine(2, wheel, SPACED)).toBeCloseTo(1, 5);
    });

    it('never decreases as the list scrolls down', () => {
        let prev = -Infinity;
        for (let y = 0; y <= listMax(G); y += 7) {
            const w = listToWheel(y, G);
            expect(w).toBeGreaterThanOrEqual(prev);
            prev = w;
        }
    });

    it('stays inside the wheel scroll range', () => {
        for (let y = -200; y <= listMax(G) + 200; y += 11) {
            const w = listToWheel(y, G);
            expect(w).toBeGreaterThanOrEqual(0);
            expect(w).toBeLessThanOrEqual(wheelMax(G));
        }
    });

    it('is constantly zero when the wheel needs no scrolling', () => {
        // The one-group search result: with no scroll range the wheel simply
        // does not move.
        for (let y = 0; y <= listMax(SHORT); y += 25) {
            expect(listToWheel(y, SHORT)).toBe(0);
        }
    });

    it('returns zero rather than NaN with no sections', () => {
        expect(listToWheel(0, EMPTY)).toBe(0);
        expect(wheelToList(0, EMPTY)).toBe(0);
        expect(listToWheel(50, EMPTY)).not.toBeNaN();
    });
});

describe('wheelToList', () => {
    it('round-trips on the interior, where neither pane is clamped', () => {
        for (let y = 50; y <= 450; y += 5) {
            expect(wheelToList(listToWheel(y, SPACED), SPACED)).toBeCloseTo(y, 5);
        }
    });

    it('round-trips exactly at both ends once the panes are padded', () => {
        // What the pads buy: the ends stop being special cases.
        const first = scrollTopForSection(0, SPACED);
        const last = scrollTopForSection(2, SPACED);
        expect(wheelToList(listToWheel(first, SPACED), SPACED)).toBeCloseTo(first, 5);
        expect(wheelToList(listToWheel(last, SPACED), SPACED)).toBeCloseTo(last, 5);
    });

    it('does not round-trip where a clamp binds, and that is the mapping', () => {
        // A wheel whose cards fit has no range at all, so every list position
        // shares one wheel position and the trip back cannot recover which.
        // Asserting an unqualified round trip would be asserting a falsehood.
        expect(listToWheel(0, SHORT)).toBe(0);
        expect(listToWheel(listMax(SHORT), SHORT)).toBe(0);
        // Both ends share the one wheel position, so at most one of them can
        // survive the trip back, and it is not the top.
        expect(wheelToList(0, SHORT)).not.toBeCloseTo(0, 5);
    });

    it('stays inside the list scroll range', () => {
        for (let w = -50; w <= wheelMax(G) + 50; w += 3) {
            const y = wheelToList(w, G);
            expect(y).toBeGreaterThanOrEqual(0);
            expect(y).toBeLessThanOrEqual(listMax(G));
        }
    });
});

describe('scrollTopForSection', () => {
    it('tapping a card centres that pack and that card', () => {
        // The regression, and the oldest bug here: the jump target aligned a
        // section's *top* to the viewport while everything else in the mapping
        // read its centre. Tapping a pack therefore left the centre line inside
        // whatever pack followed it: selecting the neighbour, scrolling the
        // wheel to a card nobody touched, and dropping the tapped pack below
        // the middle of the screen.
        for (let i = 0; i < SPACED.sections.length; i++) {
            const y = scrollTopForSection(i, SPACED);
            expect(focusedSection(y, SPACED)).toBe(i);
            expect(offLine(i, wheelFor(y, SPACED), SPACED)).toBeCloseTo(0, 5);
        }
    });

    it('centres a pack in the viewport', () => {
        // Section 1 is 200..600; a 200px viewport centred on it starts at 300.
        expect(scrollTopForSection(1, SPACED)).toBe(300);
    });

    it('clamps a section that cannot be centred rather than losing it', () => {
        expect(scrollTopForSection(2, G)).toBe(400);
        expect(scrollTopForSection(99, G)).toBe(400);
    });

    it('is zero with no sections', () => {
        expect(scrollTopForSection(0, EMPTY)).toBe(0);
    });
});

describe('the end pads', () => {
    it('are half a viewport less half a card, either side', () => {
        expect(wheelPadTop(SPACED)).toBe(70);
        expect(wheelPadBottom(SPACED)).toBe(70);
    });

    it('are zero when a card is as tall as the port, rather than negative', () => {
        expect(wheelPadTop({ ...SPACED, cardAdvance: 400 })).toBe(0);
        expect(wheelPadBottom({ ...SPACED, cardAdvance: 400 })).toBe(0);
    });

    it('let the first and last cards reach the line', () => {
        expect(offLine(0, 0, SPACED)).toBeCloseTo(0, 5);
        expect(offLine(2, wheelMax(SPACED), SPACED)).toBeCloseTo(0, 5);
        // Without them the stack sits against the top of the column and the
        // first card never arrives.
        expect(offLine(0, 0, { ...SPACED, wheelLead: 0 })).not.toBeCloseTo(0, 5);
    });

    it('give one card of wheel travel per pack', () => {
        // What makes the wheel a minimap: a flick moves it pack-by-pack rather
        // than mirroring the list's own much longer scroll.
        expect(wheelMax(SPACED) / SPACED.cardAdvance).toBe(SPACED.sections.length - 1);
    });
});

describe('present', () => {
    it('passes the driver through and derives only the other pane', () => {
        // The pane under the user's hand is never written to, or the write
        // cancels the momentum it is running on.
        const driven = present({ listScrollTop: 260, wheelScrollTop: 999, driver: 'list' }, SPACED);
        expect(driven.listScrollTop).toBe(260);
        expect(driven.wheelScrollTop).not.toBe(999);

        const other = present({ listScrollTop: 999, wheelScrollTop: 60, driver: 'wheel' }, SPACED);
        expect(other.wheelScrollTop).toBe(60);
        expect(other.listScrollTop).not.toBe(999);
    });

    it('draws every card from the position it is moving the wheel to', () => {
        // The frame the old design could not produce: the transforms describe
        // where the cards are going this frame, not where a `scroll` event says
        // they were last one.
        const f = present({ listScrollTop: 260, wheelScrollTop: 0, driver: 'list' }, SPACED);
        f.curves.forEach((c, i) => {
            expect(c).toEqual(cardCurve(i, f.wheelScrollTop, SPACED));
        });
    });

    it('highlights the card its own wheel position holds nearest the line', () => {
        // The consistency that used to be three separate reads: whatever
        // `focused` says has to be true of the wheel position published in the
        // same frame, not of the one a scroll event reported last frame.
        for (let y = 0; y <= listMax(SPACED); y += 17) {
            const f = present({ listScrollTop: y, wheelScrollTop: 0, driver: 'list' }, SPACED);
            expect(Math.abs(offLine(f.focused!, f.wheelScrollTop, SPACED))).toBeLessThanOrEqual(
                0.5001,
            );
        }
    });

    it('has a card for every section and no focus without one', () => {
        expect(
            present({ listScrollTop: 0, wheelScrollTop: 0, driver: 'list' }, SPACED).curves,
        ).toHaveLength(SPACED.sections.length);
        const empty = present({ listScrollTop: 0, wheelScrollTop: 0, driver: 'list' }, EMPTY);
        expect(empty.focused).toBeNull();
        expect(empty.curves).toEqual([]);
    });
});

describe('packBands', () => {
    /** Panes 200 tall side by side, cards spanning x=0..100, sections at x=140,
     *  in an explorer sitting at the viewport's origin. */
    const L: PaneLayout = {
        wheelTop: 0,
        wheelBottom: 200,
        listTop: 0,
        listBottom: 200,
        cardRight: 100,
        sectionLeft: 140,
        cardHeight: 52,
        cardLeft: 0,
        // The line these three cards are actually on. It agrees with
        // `wheelLead + i * cardAdvance` here, so the assertions below still
        // describe a uniformly-laid-out wheel; the fixture that does *not*
        // agree is `WOBBLY`, which is the point of it.
        cardTops: [70, 130, 190],
        width: 240,
        height: 200,
        viewportLeft: 0,
        viewportTop: 0,
    };
    const PACKS = SPACED.sections.map(s => ({
        id: s.id,
        palette: {
            chroma: `#${s.id}c`,
            refraction: `#${s.id}r`,
            surface: `#${s.id}s`,
        },
    }));
    const at = (listScrollTop: number) =>
        packBands(
            present({ listScrollTop, wheelScrollTop: 0, driver: 'list' }, SPACED),
            SPACED,
            L,
            PACKS,
        );

    /** Where card `i` is actually painted: its own box, moved by the wheel and
     *  shrunk by the rolodex curve about its trailing edge. Stated here from
     *  the fixture so the assertions below are about the card, not about
     *  however `packBands` chooses to find it. */
    const painted = (i: number, wheelScrollTop: number) => {
        const edge = cardEdge(cardCurve(i, wheelScrollTop, SPACED), L.cardHeight);
        const top = L.wheelTop + SPACED.wheelLead + i * SPACED.cardAdvance - wheelScrollTop;
        const centreY = top + L.cardHeight / 2;
        return { centreY, top: centreY + edge.top, bottom: centreY + edge.bottom };
    };

    /** A wheel whose cards are *not* where a uniform pitch would put them.
     *
     *  Its pitch is 51.6 and `cardAdvance` says 52, which is what a fractional
     *  card height looks like once `offsetTop` has rounded it: a line the
     *  stated pitch is wrong about by 0.4px per card, and by 3.2px at the ninth,
     *  the error growing with the index that made the seams read as broken
     *  further down the column than at the top of it. */
    const WOBBLY_TOPS = [70, 121.6, 173.2, 224.8, 276.4, 328, 379.6, 431.2, 482.8];
    const WOBBLY_G: WheelGeometry = {
        ...SPACED,
        cardAdvance: 52,
        wheelLead: 70,
        listScrollMax: 2000,
        wheelScrollMax: 400,
        sections: WOBBLY_TOPS.map((_, i) => ({ id: `p${i}`, top: i * 60, height: 60 })),
    };
    /** Tall enough that every one of the nine cards is inside the pane, so the
     *  assertion reaches the index where the old error was largest instead of
     *  being clipped away before it. */
    const WOBBLY_L: PaneLayout = { ...L, wheelBottom: 560, listBottom: 560, cardTops: WOBBLY_TOPS };
    const WOBBLY_PACKS = WOBBLY_G.sections.map(s => ({ id: s.id, palette: PACKS[0].palette }));

    it('leaves every card from the line the cards are measured on, not from a pitch', () => {
        // The regression. Card `i` used to be located at
        // `wheelLead + i * cardAdvance`, which multiplies whatever the pitch
        // rounded away by the pack's index, so a band that left its first card
        // cleanly was pixels off its ninth, and the join looked worse the
        // further down the column it was. Nothing here is uniform, and every
        // band still has to meet its own card.
        const frame = present({ listScrollTop: 0, wheelScrollTop: 0, driver: 'list' }, WOBBLY_G);
        const bands = packBands(frame, WOBBLY_G, WOBBLY_L, WOBBLY_PACKS);
        // Every card, including the ninth (where the old expression was
        // furthest out and the seam read as broken).
        expect(bands).toHaveLength(WOBBLY_TOPS.length);
        for (const band of bands) {
            const i = WOBBLY_PACKS.findIndex(p => p.id === band.id);
            const centre =
                WOBBLY_L.wheelTop + WOBBLY_TOPS[i] + L.cardHeight / 2 - frame.wheelScrollTop;
            const edge = cardEdge(frame.curves[i], L.cardHeight);
            expect(band.ribbon.top0).toBeCloseTo(centre + edge.top, 6);
            expect(band.ribbon.bottom0).toBeCloseTo(centre + edge.bottom, 6);
        }
    });

    it('reads neither the pitch nor the lead when locating a card', () => {
        // The property the measured line buys, stated directly: given the same
        // cards, the two numbers that used to place them cannot move a band.
        // One `frame` for both calls: `cardAdvance` also feeds the curves, so
        // re-deriving it per call would change the bands for reasons that have
        // nothing to do with where the cards are.
        const frame = present({ listScrollTop: 0, wheelScrollTop: 0, driver: 'list' }, WOBBLY_G);
        const wrong = { ...WOBBLY_G, cardAdvance: 999, wheelLead: -400 };
        expect(packBands(frame, wrong, WOBBLY_L, WOBBLY_PACKS)).toEqual(
            packBands(frame, WOBBLY_G, WOBBLY_L, WOBBLY_PACKS),
        );
    });

    it('leaves the card at its own centre, not its slot centre', () => {
        // The regression: a card's *slot* is its height plus the gap to the
        // next one, so the slot's centre sits half a gap below the card's. A
        // band drawn to the slot rides that much low against every card.
        const y = scrollTopForSection(1, SPACED);
        const wheel = listToWheel(y, SPACED);
        for (const band of at(y)) {
            const i = PACKS.findIndex(p => p.id === band.id);
            const mid = (band.ribbon.top0 + band.ribbon.bottom0) / 2;
            expect(mid).toBeCloseTo(painted(i, wheel).centreY, 5);
        }
    });

    it('leaves every card on the same vertical line, whatever its scale', () => {
        // Cards are scaled about their trailing edge, so that edge does not
        // move and a band can simply stop there. Tracking a scaled edge instead
        // (which is what a centre-anchored card forces) left a gap beside
        // every card but the focused one, widening with distance from the line.
        for (const y of [0, scrollTopForSection(1, SPACED), listMax(SPACED)]) {
            for (const band of at(y)) {
                expect(band.ribbon.x0).toBe(L.cardRight);
            }
        }
    });

    it('leaves a card without overlapping it', () => {
        // A card is tilted and scaled by the rolodex curve, so it paints as a
        // trapezoid inside a taller upright box. A band that overlapped that
        // box (as it did when both ends were nudged under what they join)
        // shows around the corners of the shape actually drawn, reading as the
        // band sitting on top of the card instead of leaving it.
        for (const band of at(scrollTopForSection(1, SPACED))) {
            expect(band.ribbon.x0).toBe(L.cardRight);
        }
    });

    it('arrives at the section without overlapping it', () => {
        // A pack's surface carries alpha, so anything painted twice is a
        // darker strip rather than the same colour again. The band used to
        // tuck a few pixels under the section to hide a subpixel seam, which
        // drew that strip down every join except the derived packs'; they
        // wear the theme's opaque greys, which hid it.
        for (const band of at(scrollTopForSection(1, SPACED))) {
            expect(band.ribbon.x1).toBe(L.sectionLeft);
        }
    });

    it('leaves at the card height the curve is painting', () => {
        // Not the height of the box a tilted card reports: the band has to
        // match the card as drawn, and the scale is known without asking.
        const band = at(scrollTopForSection(1, SPACED)).find(b => b.id === 'b')!;
        const scale = cardCurve(1, listToWheel(scrollTopForSection(1, SPACED), SPACED), SPACED)
            .scale;
        expect(band.ribbon.bottom0 - band.ribbon.top0).toBeCloseTo(L.cardHeight * scale, 5);
    });

    it('draws one band per pack on screen and none for the rest', () => {
        // Continuity comes from the set changing one pack at a time, each
        // shrinking to nothing before it goes.
        const centred = at(scrollTopForSection(1, SPACED));
        expect(centred.map(b => b.id)).toContain('b');
        for (const band of centred) {
            expect(band.ribbon.bottom1).toBeGreaterThan(band.ribbon.top1);
            expect(band.ribbon.bottom0).toBeGreaterThan(band.ribbon.top0);
        }
    });

    it('trims a band to both panes rather than drawing outside them', () => {
        for (const band of at(scrollTopForSection(2, SPACED))) {
            expect(band.ribbon.top0).toBeGreaterThanOrEqual(L.wheelTop);
            expect(band.ribbon.bottom0).toBeLessThanOrEqual(L.wheelBottom);
            expect(band.ribbon.top1).toBeGreaterThanOrEqual(L.listTop);
            expect(band.ribbon.bottom1).toBeLessThanOrEqual(L.listBottom);
        }
    });

    it('carries each pack its own colour and the fade its card is under', () => {
        const frame = present(
            { listScrollTop: scrollTopForSection(0, SPACED), wheelScrollTop: 0, driver: 'list' },
            SPACED,
        );
        for (const band of packBands(frame, SPACED, L, PACKS)) {
            const i = PACKS.findIndex(p => p.id === band.id);
            expect(band.palette).toEqual(PACKS[i].palette);
            expect(band.opacity).toBe(frame.curves[i].opacity);
        }
    });
});

describe('the projection ribbon', () => {
    const R = { x0: 0, top0: 40, bottom0: 80, x1: 100, top1: 0, bottom1: 200 };

    it('leaves at the card extent and arrives at the section extent', () => {
        // The compression the wheel performs, made visible: 40px of card
        // opening onto 200px of section.
        const d = ribbonPath(R);
        expect(d.startsWith('M 0 40')).toBe(true);
        expect(d).toContain('100 0');
        expect(d).toContain('L 100 200');
        expect(d.endsWith('Z')).toBe(true);
    });

    it('bends both edges about the same midline', () => {
        // Control points off the same abscissa are what keep the band an even
        // thickness through the turn instead of pinching on one edge.
        expect(ribbonPath(R)).toContain('C 50 40, 50 0,');
        expect(ribbonPath(R)).toContain('C 50 200, 50 80,');
    });

    it('rims the band along both edges and nowhere else', () => {
        // Two closed subpaths, one inside each edge, not one region with a
        // hole, which is what a fill rule would have to resolve.
        const d = ribbonRimPath(R, 2);
        expect(d.match(/M /g)).toHaveLength(2);
        expect(d.match(/Z/g)).toHaveLength(2);
        // Each strip's outer boundary *is* the ribbon's, so the rim and the
        // fill beneath it end on the same curve.
        expect(d).toContain('M 0 40 C 50 40, 50 0, 100 0');
        expect(d).toContain('C 50 200, 50 80, 0 80 Z');
        // And each is a rim thick, taken inward.
        expect(d).toContain('L 100 2 C 50 2, 50 42, 0 42 Z');
        expect(d).toContain('M 0 78 C 50 78, 50 198, 100 198');
    });

    it('leaves both ends of the band open', () => {
        // The card end and the section end are interior to the pack. A cap
        // across either would rule a line down the join the projection exists
        // to make continuous, so every vertical run in the rim is the rim's own
        // thickness, where the fill's is the whole height it arrives at.
        const spans = (d: string) =>
            [...d.matchAll(/100 (-?[\d.]+) L 100 (-?[\d.]+)/g)].map(m =>
                Math.abs(Number(m[2]) - Number(m[1])),
            );
        expect(spans(ribbonPath(R))).toEqual([200]);
        expect(spans(ribbonRimPath(R, 2))).toEqual([2, 2]);
    });

    it('hands the band over from rim to core along one curve', () => {
        // The two carry different strengths of the same light, so they have to
        // meet exactly: a gap would show as a dark line inside the rim, an
        // overlap as a bright one.
        expect(ribbonCorePath(R, 2)).toBe(
            'M 0 42 C 50 42, 50 2, 100 2 L 100 198 C 50 198, 50 78, 0 78 Z',
        );
        const rim = ribbonRimPath(R, 2);
        expect(rim).toContain('C 50 2, 50 42, 0 42 Z');
        expect(rim).toContain('M 0 78 C 50 78, 50 198,');
    });

    it('lets the core cross itself where the band is thinner than two rims', () => {
        // A pack nearly scrolled out arrives 1px tall, which is less than the
        // rims take from either side. The core inverts rather than clamping:
        // the rims have already closed over that end, and the ribbon's own clip
        // bounds whatever the crossing describes.
        const pinched = { x0: 0, top0: 40, bottom0: 80, x1: 100, top1: 100, bottom1: 101 };
        const core = ribbonCorePath(pinched, 2);
        expect(core).toContain('100 102');
        expect(core).toContain('L 100 99');
    });

    it('winds both strips the same way, so a pinched band still fills', () => {
        // Where the band is thinner than twice the rim the two strips overlap,
        // and opposite winding would cancel them out under the nonzero rule.
        // Both therefore run left to right along their upper edge.
        const strips = ribbonRimPath(R, 2).split('M ').filter(Boolean);
        expect(strips).toHaveLength(2);
        for (const s of strips) {
            expect(s.startsWith('0 ')).toBe(true);
            expect(s).toContain('100 ');
        }
    });

    it('trims a section to what the list is showing', () => {
        // A pack taller than the viewport: the ribbon must arrive at the
        // scrollport's edge, not run off it onto the search field.
        expect(visibleSpan(-500, 900, 0, 400)).toEqual({ top: 0, bottom: 400 });
        expect(visibleSpan(100, 250, 0, 400)).toEqual({ top: 100, bottom: 250 });
    });

    it('has nothing to draw for a section scrolled off-screen', () => {
        expect(visibleSpan(600, 900, 0, 400)).toBeNull();
        // Touching the edge is not showing: a zero-height band is no band.
        expect(visibleSpan(400, 900, 0, 400)).toBeNull();
    });
});

describe('cardCurve', () => {
    it('is flat on the focus line', () => {
        const c = cardCurve(0, 0, SPACED);
        expect(c.t).toBeCloseTo(0, 5);
        expect(c.rotateX).toBeCloseTo(0, 5);
        expect(c.scale).toBeCloseTo(1, 5);
        expect(c.opacity).toBeCloseTo(1, 5);
    });

    it('is symmetric about the line', () => {
        // Card 1 on the line, its neighbours one card either side of it.
        const onCard1 = wheelFor(scrollTopForSection(1, SPACED), SPACED);
        const above = cardCurve(0, onCard1, SPACED);
        const below = cardCurve(2, onCard1, SPACED);
        expect(above.t).toBeCloseTo(-below.t, 5);
        expect(above.scale).toBeCloseTo(below.scale, 5);
        expect(above.opacity).toBeCloseTo(below.opacity, 5);
    });

    it('never inverts or disappears a card entirely', () => {
        for (let i = 0; i < 3; i++) {
            for (let w = 0; w <= wheelMax(SPACED); w += 5) {
                const c = cardCurve(i, w, SPACED);
                expect(c.scale).toBeGreaterThan(0);
                expect(c.opacity).toBeGreaterThan(0);
                expect(Math.abs(c.t)).toBeLessThanOrEqual(1);
            }
        }
    });
});

describe('sameGeometry', () => {
    it('holds a layout still through changes no layout could express', () => {
        // Fractional metrics chatter where whole-px ones rounded the noise
        // away. Below one LayoutUnit there is nothing to redraw, and treating
        // it as a change keeps the measure/render loop awake for nothing.
        expect(sameGeometry(SPACED, SPACED)).toBe(true);
        expect(sameGeometry({ ...SPACED, cardAdvance: 60 + 1 / 128 }, SPACED)).toBe(true);
        expect(sameGeometry({ ...SPACED, cardAdvance: 60 + 1 / 32 }, SPACED)).toBe(false);
    });

    it('notices a section moving, resizing, or changing identity', () => {
        const moved = SPACED.sections.map((s, i) => (i === 1 ? { ...s, top: s.top + 1 } : s));
        const renamed = SPACED.sections.map((s, i) => (i === 1 ? { ...s, id: 'z' } : s));
        expect(sameGeometry({ ...SPACED, sections: moved }, SPACED)).toBe(false);
        expect(sameGeometry({ ...SPACED, sections: renamed }, SPACED)).toBe(false);
        expect(sameGeometry({ ...SPACED, sections: SPACED.sections.slice(1) }, SPACED)).toBe(false);
    });

    it('is never the same as nothing measured yet', () => {
        expect(sameGeometry(SPACED, null)).toBe(false);
    });
});
