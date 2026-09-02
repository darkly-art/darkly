import { describe, it, expect, vi } from 'vitest';
import { beginScrubDrag } from '../scrubDrag';

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
