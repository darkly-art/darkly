import { describe, expect, it, vi } from 'vitest';
import { ReadbackQueue } from '../readbacks';

/** A poll that returns `null` for the first `nullFrames` calls, then the value.
 *  Models the real `poll_*_result` contract: nothing until the GPU readback
 *  lands, then the payload exactly once. */
function pollAfter<T>(nullFrames: number, value: T) {
    let calls = 0;
    return vi.fn(async () => (calls++ < nullFrames ? null : value));
}

/** Let queued promise callbacks run. The queue settles its entries from a
 *  `.then`, so a poll's effect is visible on the next microtask, not the
 *  statement after `poll()`. */
const flush = () => new Promise((r) => setTimeout(r, 0));

describe('ReadbackQueue', () => {
    it('resolves with the first non-null poll result', async () => {
        const q = new ReadbackQueue();
        const poll = pollAfter(0, { width: 4, height: 2 });
        const result = q.awaitResult('copy', poll);
        q.poll();
        await expect(result).resolves.toEqual({ width: 4, height: 2 });
        expect(q.pending).toBe(0);
    });

    it('stays pending across null polls and re-polls on later frames', async () => {
        const q = new ReadbackQueue();
        const poll = pollAfter(2, 'landed');
        const result = q.awaitResult('copy', poll);

        q.poll();
        await flush();
        expect(q.pending).toBe(1);
        q.poll();
        await flush();
        expect(q.pending).toBe(1);
        q.poll();

        await expect(result).resolves.toBe('landed');
        expect(poll).toHaveBeenCalledTimes(3);
    });

    it('does not stack a second request on a poll that has not come back', async () => {
        const q = new ReadbackQueue();
        // Never settles: models a readback whose transport round trip outlives
        // the frame that issued it.
        const poll = vi.fn(() => new Promise<string | null>(() => {}));
        void q.awaitResult('copy', poll);

        q.poll();
        q.poll();
        q.poll();

        expect(poll).toHaveBeenCalledTimes(1);
    });

    it('resolves two readbacks on different slots independently', async () => {
        const q = new ReadbackQueue();
        const slow = q.awaitResult('save', pollAfter(1, 'save'));
        const fast = q.awaitResult('copy', pollAfter(0, 'copy'));

        q.poll();
        await expect(fast).resolves.toBe('copy');
        expect(q.pending).toBe(1);

        q.poll();
        await expect(slow).resolves.toBe('save');
        expect(q.pending).toBe(0);
    });

    it('reports the outstanding count, which is what keeps the frame loop alive', async () => {
        const q = new ReadbackQueue();
        expect(q.pending).toBe(0);
        const a = q.awaitResult('copy', pollAfter(0, 1));
        const b = q.awaitResult('export', pollAfter(0, 2));
        expect(q.pending).toBe(2);
        q.poll();
        await Promise.all([a, b]);
        expect(q.pending).toBe(0);
    });

    it('rejects the caller when the poll request itself fails', async () => {
        const q = new ReadbackQueue();
        const result = q.awaitResult('copy', async () => {
            throw new Error('engine_error');
        });
        q.poll();
        await expect(result).rejects.toThrow('engine_error');
        expect(q.pending).toBe(0);
    });

    it('abort() rejects everything outstanding, so a save on a closed tab fails rather than hangs', async () => {
        const q = new ReadbackQueue();
        const save = q.awaitResult('save', pollAfter(99, 'never'));
        const exportJob = q.awaitResult('export', pollAfter(99, 'never'));

        q.poll();
        q.abort();

        await expect(save).rejects.toThrow('disposed');
        await expect(exportJob).rejects.toThrow('disposed');
        expect(q.pending).toBe(0);
    });

    it('a poll landing after abort() does not re-settle the rejected promise', async () => {
        const q = new ReadbackQueue();
        let release: (v: string | null) => void = () => {};
        const result = q.awaitResult('copy', () => new Promise<string | null>((r) => (release = r)));

        q.poll();
        q.abort();
        await expect(result).rejects.toThrow('disposed');

        // The in-flight poll comes back with a real value after the tab closed.
        // Settling is one-shot, so this is dropped rather than throwing
        // "resolve after reject" or resurrecting the entry.
        release('late');
        await flush();
        expect(q.pending).toBe(0);
    });
});

// The engine keeps one pending result per kind and `poll_*_result` takes it, so
// two waiters on one slot cannot both be served: the second start overwrites
// the first's result. Left unmodelled, the loser waits forever and its entry
// keeps `pending` non-zero, which keeps the frame loop rescheduling for the
// life of the tab. Double-tapping copy is all it takes.
describe('two readbacks on one slot', () => {
    it('supersede rather than both waiting, and the loser is told', async () => {
        const q = new ReadbackQueue();
        const first = q.awaitResult('copy', pollAfter(99, 'never'));
        const second = q.awaitResult('copy', pollAfter(0, 'second'));

        await expect(first).rejects.toThrow('superseded');
        expect(q.pending).toBe(1);

        q.poll();
        await expect(second).resolves.toBe('second');
        expect(q.pending).toBe(0);
    });

    it('a superseded poll landing late does not clear the live entry', async () => {
        const q = new ReadbackQueue();
        let release: (v: string | null) => void = () => {};
        const first = q.awaitResult('copy', () => new Promise<string | null>((r) => (release = r)));
        q.poll();
        const second = q.awaitResult('copy', pollAfter(99, 'never'));
        await expect(first).rejects.toThrow('superseded');

        // The superseded request comes back with a real value afterwards. It
        // must not settle or evict the entry that replaced it.
        release('stale');
        await flush();

        expect(q.pending).toBe(1);
        q.abort();
        await expect(second).rejects.toThrow('disposed');
    });
});
