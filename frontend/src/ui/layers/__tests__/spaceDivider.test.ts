import { describe, it, expect } from 'vitest';
import { gapAt, maxEligible } from '../spaceDivider';

/** Rows of deliberately unequal height — a plain layer, a group with two
 *  nested children, a layer carrying a mask sub-row. */
const rows = [
    { top: 0, height: 24 },
    { top: 24, height: 72 },
    { top: 96, height: 48 },
];

describe('gapAt', () => {
    // Regression: the first divider positioned itself at `count * 34px`, so
    // with rows of any other height it drifted away from the gaps entirely and
    // could come to rest through the middle of a row.
    it('resolves gaps against real row heights, not an assumed one', () => {
        expect(gapAt(0, rows, 3)).toBe(0);
        // Past the first row's midpoint (12) but not the second's (60).
        expect(gapAt(20, rows, 3)).toBe(1);
        expect(gapAt(59, rows, 3)).toBe(1);
        // Past the tall middle row's midpoint.
        expect(gapAt(61, rows, 3)).toBe(2);
        // Past everything.
        expect(gapAt(500, rows, 3)).toBe(3);
    });

    it('never exceeds the eligible run', () => {
        expect(gapAt(500, rows, 1)).toBe(1);
        expect(gapAt(500, rows, 0)).toBe(0);
    });

    it('skips rows that render no gap to land in', () => {
        const collapsed = [
            { top: 0, height: 24 },
            { top: 24, height: 0 },
            { top: 24, height: 24 },
        ];
        expect(gapAt(30, collapsed, 3)).toBe(1);
        expect(gapAt(40, collapsed, 3)).toBe(2);
    });
});

describe('maxEligible', () => {
    it('stops at the first row that cannot render in screen space', () => {
        expect(
            maxEligible([
                { screenSpaceEligible: true },
                { screenSpaceEligible: true },
                { screenSpaceEligible: false },
                { screenSpaceEligible: true },
            ]),
        ).toBe(2);
        expect(maxEligible([{ screenSpaceEligible: false }])).toBe(0);
        expect(maxEligible([])).toBe(0);
    });
});
