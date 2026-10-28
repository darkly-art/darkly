import { describe, it, expect, vi } from 'vitest';
import { beginScrubDrag, scrubValue, SCRUB_TRAVEL_PX } from '../scrubDrag';

/** A drag harness whose value is just the x coordinate, so assertions read
 *  directly as pointer positions. */
function harness() {
    const onPreview = vi.fn();
    const onCommit = vi.fn();
    const onFinish = vi.fn();
    const drag = beginScrubDrag({
        toValue: (clientX) => clientX,
        onPreview,
        onCommit,
        onFinish,
    });
    return { drag, onPreview, onCommit, onFinish };
}

describe('beginScrubDrag', () => {
    it('previews every move but commits once, on release', () => {
        const { drag, onPreview, onCommit } = harness();

        for (const x of [10, 20, 30, 40, 50]) drag.move(x, 0);
        expect(onPreview).toHaveBeenCalledTimes(5);
        expect(onCommit).not.toHaveBeenCalled();

        drag.end();
        expect(onCommit).toHaveBeenCalledTimes(1);
        expect(onCommit).toHaveBeenCalledWith(50);
    });

    it('commits nothing when the pointer never moved', () => {
        const { drag, onCommit, onFinish } = harness();
        drag.end();
        expect(onCommit).not.toHaveBeenCalled();
        expect(onFinish).toHaveBeenCalledTimes(1);
    });

    it('commits the previewed value when capture is lost mid-drag', () => {
        // `lostpointercapture` routes to the same `end`. Discarding here would
        // leave the caller's local state showing a value never committed.
        const { drag, onCommit } = harness();
        drag.move(10, 0);
        drag.move(25, 0);
        drag.end();
        expect(onCommit).toHaveBeenCalledTimes(1);
        expect(onCommit).toHaveBeenCalledWith(25);
    });

    it('is idempotent: pointerup and lostpointercapture both landing commit once', () => {
        const { drag, onCommit, onFinish } = harness();
        drag.move(15, 0);
        drag.end();
        drag.end();
        expect(onCommit).toHaveBeenCalledTimes(1);
        expect(onFinish).toHaveBeenCalledTimes(1);
    });

    it('ignores moves after the drag has ended', () => {
        const { drag, onPreview, onCommit } = harness();
        drag.move(15, 0);
        drag.end();
        drag.move(99, 0);
        expect(onPreview).toHaveBeenCalledTimes(1);
        expect(onCommit).toHaveBeenCalledWith(15);
    });

    it('runs onFinish exactly once so paired acquire/release stays balanced', () => {
        const { drag, onFinish } = harness();
        drag.move(5, 0);
        drag.end();
        drag.end();
        expect(onFinish).toHaveBeenCalledTimes(1);
    });
});

describe('scrubValue', () => {
    // Regression: Size spans 0.01%..400% and moved 1% per pixel, so 1% to 3%
    // was a 2 px target. Its declared curve spends the drag on the low end.
    it('makes 1% to 3% of size a drag of at least 20 px', () => {
        const curve = { exponent: 3, reversed: false };
        expect(scrubValue(1, 20, 0.01, 400, curve)).toBeLessThan(3);
    });

    it('is the plain linear drag when the curve is linear', () => {
        const linear = { exponent: 1, reversed: false };
        for (const [start, dx, min, max] of [
            [0.5, 40, 0, 1],
            [10, -25, -100, 100],
            [3, 7, 1, 512],
        ]) {
            expect(scrubValue(start, dx, min, max, linear)).toBeCloseTo(
                start + (dx * (max - min)) / SCRUB_TRAVEL_PX,
                9,
            );
            expect(scrubValue(start, dx, min, max)).toBeCloseTo(
                start + (dx * (max - min)) / SCRUB_TRAVEL_PX,
                9,
            );
        }
    });

    it('clamps to the range', () => {
        const curve = { exponent: 3, reversed: false };
        expect(scrubValue(1, -1000, 0.01, 400, curve)).toBe(0.01);
        expect(scrubValue(1, 1000, 0.01, 400, curve)).toBe(400);
    });
});
