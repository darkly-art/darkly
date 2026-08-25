import { describe, it, expect } from 'vitest';
import { ScrollSyncToken } from '../scrollSync';

type Side = 'list' | 'wheel';

describe('ScrollSyncToken', () => {
    it('the first claimer wins and the other side is refused', () => {
        const t = new ScrollSyncToken<Side>(120);
        expect(t.claim('list', 0)).toBe(true);
        // The echo of our own write to the wheel.
        expect(t.claim('wheel', 5)).toBe(false);
    });

    it('the owner re-claiming restarts the hold, so a fling keeps it', () => {
        const t = new ScrollSyncToken<Side>(120);
        t.claim('list', 0);
        // Momentum events every 16ms for well past one hold window.
        for (let now = 16; now <= 400; now += 16) {
            expect(t.claim('list', now)).toBe(true);
            expect(t.claim('wheel', now)).toBe(false);
        }
    });

    it('the other side can claim once the owner goes quiet', () => {
        const t = new ScrollSyncToken<Side>(120);
        t.claim('list', 0);
        expect(t.claim('wheel', 119)).toBe(false);
        expect(t.claim('wheel', 120)).toBe(true);
    });

    it('a pointerdown preempts mid-hold rather than being eaten', () => {
        // A pen landing on the wheel during the list's momentum tail. Without
        // preemption the token refuses it for the rest of the window and the
        // wheel snaps back under the painter's finger.
        const t = new ScrollSyncToken<Side>(120);
        t.claim('list', 0);
        expect(t.claim('wheel', 10)).toBe(false);

        t.preempt('wheel', 10);
        expect(t.owner).toBe('wheel');
        expect(t.claim('wheel', 11)).toBe(true);
        // And the list's remaining momentum is now the echo.
        expect(t.claim('list', 12)).toBe(false);
    });

    it('release frees it immediately', () => {
        const t = new ScrollSyncToken<Side>(120);
        t.claim('list', 0);
        t.release();
        expect(t.owner).toBeNull();
        expect(t.claim('wheel', 1)).toBe(true);
    });

    it('an unowned token grants whoever asks first', () => {
        const t = new ScrollSyncToken<Side>(120);
        expect(t.claim('wheel', 999)).toBe(true);
        expect(t.owner).toBe('wheel');
    });
});
