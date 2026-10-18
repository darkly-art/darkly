/**
 * Tap-to-cycle over a most-recently-used list.
 *
 * One press steps to the most recent item other than the current one. Further
 * presses in quick succession keep stepping back through the list, wrapping
 * around to where they started. Stepping the other way walks the same ring in
 * reverse, starting from its far end, and the two directions mix freely within
 * one chain. A pause (or `settle()`) ends the chain and commits the item it
 * landed on as the most recent, so the next press after a pause swaps straight
 * back.
 *
 * The list is frozen when a chain starts and nothing is committed while it is
 * alive. Committing on every step would put each landed item at the head and
 * rapid presses would only ever bounce between the first two.
 *
 * Pure: no runes, no DOM, no engine. The caller decides what landing on an
 * item means (`step()` returns it) and what committing means (`commit`).
 */

export interface RecentCycleDeps<T> {
    /** The live MRU list, newest first. Snapshotted when a chain starts. */
    items: () => readonly T[];
    /** What is active now, or null when nothing in the list is. */
    current: () => T | null;
    /** Record `item` as most recently used; called once, when a chain ends. */
    commit: (item: T) => void;
    /** How many items a chain walks, counting the current one; the rest of
     *  the list is left out of the walk. Read when a chain starts. */
    limit: () => number;
    /** Quiet time after a step that ends the chain. */
    pauseMs: number;
}

/** Which way a press walks the ring: `1` toward older items, `-1` back toward
 *  newer ones, which from the start of a chain means the oldest it holds. */
export type CycleDirection = 1 | -1;

export interface RecentCycle<T> {
    /** Whether a press would land somewhere: the list holds something other
     *  than the current item. */
    canStep(): boolean;
    /** One press: start a chain or advance the live one in `direction`.
     *  Returns what to switch to, or null when there is nothing to land on (an
     *  empty list, or a one-entry chain that already landed and only re-armed
     *  its timer). */
    step(direction?: CycleDirection): T | null;
    /** End the chain now, committing what it landed on. No-op without one. */
    settle(): void;
    /** Whether a chain is alive. */
    readonly active: boolean;
}

interface Chain<T> {
    ring: T[];
    cursor: number;
    landed: T | null;
}

export function createRecentCycle<T>(deps: RecentCycleDeps<T>): RecentCycle<T> {
    let chain: Chain<T> | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;

    /** The ring a new chain walks: the current item first, so the first step
     *  lands on the most recent other one and a full loop comes back to it,
     *  cut to `limit` entries. */
    function ring(): { ring: T[]; current: T | null } {
        const current = deps.current();
        const items = deps.items();
        const all = current === null ? [...items] : [current, ...items.filter(i => i !== current)];
        return { ring: all.slice(0, Math.max(1, deps.limit())), current };
    }

    function rearm() {
        if (timer !== null) clearTimeout(timer);
        timer = setTimeout(settle, deps.pauseMs);
    }

    function canStep(): boolean {
        const { ring: r, current } = ring();
        return r.some(i => i !== current);
    }

    function step(direction: CycleDirection = 1): T | null {
        if (!chain) {
            if (!canStep()) return null;
            const { ring: r, current } = ring();
            // With something current it sits at index 0 and the first step
            // moves off it either way. With nothing current the chain starts
            // between the two ends, so the first step lands on the head going
            // forward and on the far end going back.
            const cursor = current !== null ? 0 : direction === 1 ? -1 : r.length;
            chain = { ring: r, cursor, landed: null };
        }
        const n = chain.ring.length;
        chain.cursor = (((chain.cursor + direction) % n) + n) % n;
        const next = chain.ring[chain.cursor];
        rearm();
        if (next === chain.landed) return null;
        chain.landed = next;
        return next;
    }

    function settle() {
        if (timer !== null) {
            clearTimeout(timer);
            timer = null;
        }
        const landed = chain?.landed ?? null;
        chain = null;
        if (landed !== null) deps.commit(landed);
    }

    return {
        canStep,
        step,
        settle,
        get active() {
            return chain !== null;
        },
    };
}
