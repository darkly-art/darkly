import { describe, it, expect } from 'vitest';
import {
    sectionAt,
    listToWheel,
    wheelToList,
    scrollTopForSection,
    focusedSection,
    cardCurve,
    listMax,
    wheelMax,
    type WheelGeometry,
    type SectionExtent,
} from '../wheel';

/** Sections of deliberately uneven height, which is the whole point of a
 *  piecewise map: 100 / 400 / 100 against a uniform 60px card. */
const SECTIONS: SectionExtent[] = [
    { id: 'a', top: 0, height: 100 },
    { id: 'b', top: 100, height: 400 },
    { id: 'c', top: 500, height: 100 },
];

/** Both panes scrollable: 600px of list content in a 200px port (max 400), and
 *  180px of wheel content in a 120px port (max 60). The scroll maxima are given
 *  rather than derived, as the component reads them from the DOM. */
const G: WheelGeometry = {
    cardAdvance: 60,
    wheelViewport: 120,
    listViewport: 200,
    listScrollMax: 400,
    wheelScrollMax: 60,
    sections: SECTIONS,
};

/** A wheel whose cards fit, so it has no scroll range of its own. */
const SHORT: WheelGeometry = { ...G, wheelViewport: 400, wheelScrollMax: 0 };

const EMPTY: WheelGeometry = { ...G, sections: [], listScrollMax: 0, wheelScrollMax: 0 };

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

describe('listToWheel', () => {
    it('maps uneven sections onto uniform cards', () => {
        // Centre of section 1 (list content 300) is card index 1.5, so the
        // wheel wants 90 under its centre: 90 - 60 = 30.
        const listScrollTop = 300 - G.listViewport / 2;
        expect(listToWheel(listScrollTop, G)).toBeCloseTo(30, 5);
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
        // The one-group search result, and decision 11's inert case: with no
        // scroll range the wheel simply does not move.
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
        for (let y = 120; y <= 260; y += 5) {
            expect(wheelToList(listToWheel(y, G), G)).toBeCloseTo(y, 5);
        }
    });

    it('does not round-trip where a clamp binds, and that is the mapping', () => {
        // A tall *first* section: the list's centre sits inside it while the
        // wheel is already pinned at 0, so a whole range of list positions
        // share one wheel position and the trip back cannot recover which.
        // Asserting an unqualified round trip would be asserting a falsehood.
        const TALL: WheelGeometry = {
            cardAdvance: 60,
            wheelViewport: 120,
            listViewport: 200,
            listScrollMax: 400,
            wheelScrollMax: 60,
            sections: [
                { id: 'a', top: 0, height: 400 },
                { id: 'b', top: 400, height: 100 },
                { id: 'c', top: 500, height: 100 },
            ],
        };
        expect(listToWheel(0, TALL)).toBe(0);
        expect(listToWheel(50, TALL)).toBe(0);
        expect(wheelToList(0, TALL)).not.toBeCloseTo(50, 5);
    });

    it('round-trips exactly at both ends when no clamp binds', () => {
        expect(wheelToList(listToWheel(0, G), G)).toBeCloseTo(0, 5);
        expect(wheelToList(listToWheel(listMax(G), G), G)).toBeCloseTo(listMax(G), 5);
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
    it('aligns a section heading to the top of the list viewport', () => {
        expect(scrollTopForSection(1, G)).toBe(100);
    });

    it('clamps the last section so it can still be reached', () => {
        expect(scrollTopForSection(2, G)).toBe(400);
        expect(scrollTopForSection(99, G)).toBe(400);
    });

    it('is zero with no sections', () => {
        expect(scrollTopForSection(0, EMPTY)).toBe(0);
    });
});

describe('focusedSection', () => {
    it('tracks the viewport centre, not its top', () => {
        // scrollTop 0 shows content 0..200; its centre (100) is section 1.
        expect(focusedSection(0, G)).toBe(1);
    });

    it('agrees with listToWheel about which section is focused', () => {
        // The regression for a mapping anchored on the top while the highlight
        // reads the centre: the highlighted card must be the one the wheel
        // scrolled to.
        for (let y = 0; y <= listMax(G); y += 13) {
            const focused = focusedSection(y, G)!;
            const wheelCentre = listToWheel(y, G) + G.wheelViewport / 2;
            const cardUnderCentre = Math.floor(wheelCentre / G.cardAdvance);
            // Equal except where the wheel is clamped at an end and cannot
            // travel far enough to centre the focused card.
            const clamped = listToWheel(y, G) === 0 || listToWheel(y, G) === wheelMax(G);
            if (!clamped) expect(cardUnderCentre).toBe(focused);
        }
    });

    it('is null with no sections', () => {
        expect(focusedSection(0, EMPTY)).toBeNull();
    });
});

describe('cardCurve', () => {
    it('is flat at the scrollport centre', () => {
        // Card 1 spans 60..120, centred at 90; a 120px port at scrollTop 30 is
        // centred at 90 too.
        const c = cardCurve(1, 30, G);
        expect(c.t).toBeCloseTo(0, 5);
        expect(c.rotateX).toBeCloseTo(0, 5);
        expect(c.scale).toBeCloseTo(1, 5);
    });

    it('is symmetric about the centre', () => {
        const above = cardCurve(0, 30, G);
        const below = cardCurve(2, 30, G);
        expect(above.t).toBeCloseTo(-below.t, 5);
        expect(above.scale).toBeCloseTo(below.scale, 5);
        expect(above.opacity).toBeCloseTo(below.opacity, 5);
    });

    it('never inverts or disappears a card entirely', () => {
        for (let i = 0; i < 3; i++) {
            for (let w = 0; w <= wheelMax(G); w += 5) {
                const c = cardCurve(i, w, G);
                expect(c.scale).toBeGreaterThan(0);
                expect(c.opacity).toBeGreaterThan(0);
                expect(Math.abs(c.t)).toBeLessThanOrEqual(1);
            }
        }
    });
});
