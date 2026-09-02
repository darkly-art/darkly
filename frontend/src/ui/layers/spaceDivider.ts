/**
 * Where the viewport divider lands for a given pointer position.
 *
 * Split out of the component because it is the only real logic in it, and
 * because the bug it exists to prevent is arithmetic: the first version
 * positioned the line at `count * ROW_HEIGHT` with a constant row height, which
 * floats free of the rows as soon as they are not all that tall — groups nest,
 * modifiers add sub-rows — and lands the line through the middle of one.
 *
 * Measuring against the rows that are actually on screen has no such
 * assumption.
 */

/** The vertical span of one rendered row. */
export interface RowSpan {
    top: number;
    height: number;
}

/**
 * How many rows sit above `clientY`, clamped to `maxCount`.
 *
 * A row counts as above once the pointer passes its midpoint, so the line
 * follows the cursor into the gap the user is aiming at rather than snapping
 * only on full-row crossings. Zero-height rows (a collapsed group's children)
 * are skipped: they occupy no gap to land in.
 */
export function gapAt(clientY: number, rows: RowSpan[], maxCount: number): number {
    let n = 0;
    for (const row of rows) {
        if (row.height === 0) continue;
        if (clientY < row.top + row.height / 2) break;
        n++;
    }
    return Math.max(0, Math.min(maxCount, n));
}

/**
 * How far down the divider may go: it stops at the first row that cannot be
 * rendered after the view transform. The engine clamps authoritatively when the
 * drag lands; this is what keeps the handle from visibly overshooting under the
 * cursor.
 */
export function maxEligible(rows: { screenSpaceEligible?: boolean }[]): number {
    let n = 0;
    for (const row of rows) {
        if (!row.screenSpaceEligible) break;
        n++;
    }
    return n;
}
