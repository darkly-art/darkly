/**
 * Tap-to-cycle over a most-recently-used list.
 *
 * One press steps to the most recent item other than the current one. Further
 * presses in quick succession keep stepping back through the list, wrapping
 * around to where they started. A pause (or `settle()`) ends the chain and
 * commits the item it landed on as the most recent, so the next press after a
 * pause swaps straight back.
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
    /** Quiet time after a step that ends the chain. */
    pauseMs: number;
}

export interface RecentCycle<T> {
    /** Whether a press would land somewhere: the list holds something other
     *  than the current item. */
    canStep(): boolean;
    /** One press: start a chain or advance the live one. Returns what to
     *  switch to, or null when there is nothing to land on (an empty list, or
     *  a one-entry chain that already landed and only re-armed its timer). */
    step(): T | null;
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
     *  lands on the most recent other one and a full loop comes back to it. */
    function ring(): { ring: T[]; current: T | null } {
        const current = deps.current();
        const items = deps.items();
        if (current === null) return { ring: [...items], current };
        return { ring: [current, ...items.filter(i => i !== current)], current };
    }

    function rearm() {
        if (timer !== null) clearTimeout(timer);
        timer = setTimeout(settle, deps.pauseMs);
    }

    function canStep(): boolean {
        const { ring: r, current } = ring();
        return r.some(i => i !== current);
    }

    function step(): T | null {
        if (!chain) {
            if (!canStep()) return null;
            const { ring: r, current } = ring();
            // With something current it sits at index 0 and the first step
            // moves past it; with nothing current the first step is the head.
            chain = { ring: r, cursor: current === null ? -1 : 0, landed: null };
        }
        chain.cursor = (chain.cursor + 1) % chain.ring.length;
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
