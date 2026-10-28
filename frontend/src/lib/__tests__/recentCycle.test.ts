import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createRecentCycle } from '../recentCycle';

const PAUSE = 700;

/** A live MRU list: `commit` reorders it exactly as `recentBrushes.use` does,
 *  so a commit that happened mid-chain would show up in every later step. */
function mru(
    initial: string[],
    current: (list: string[]) => string | null = l => l[0] ?? null,
    limit = Infinity,
) {
    let list = [...initial];
    const commit = vi.fn((id: string) => {
        list = [id, ...list.filter(x => x !== id)];
    });
    const cycle = createRecentCycle<string>({
        items: () => list,
        current: () => current(list),
        commit,
        limit: () => limit,
        pauseMs: PAUSE,
    });
    return { cycle, commit, list: () => list };
}

beforeEach(() => {
    vi.useFakeTimers();
});
afterEach(() => {
    vi.useRealTimers();
});

describe('recent cycle', () => {
    it('first_press_lands_on_the_most_recent_other_item', () => {
        const { cycle } = mru(['a', 'b', 'c']);
        expect(cycle.step()).toBe('b');
    });

    it('rapid_presses_walk_the_ring_and_wrap', () => {
        const { cycle } = mru(['a', 'b', 'c']);
        expect([cycle.step(), cycle.step(), cycle.step(), cycle.step()]).toEqual(['b', 'c', 'a', 'b']);
    });

    it('nothing_is_committed_while_the_chain_is_alive', () => {
        const { cycle, commit } = mru(['a', 'b', 'c']);
        cycle.step();
        cycle.step();
        cycle.step();
        expect(commit).not.toHaveBeenCalled();
        expect(cycle.active).toBe(true);
    });

    it('the_pause_commits_the_landed_item_once_and_ends_the_chain', () => {
        const { cycle, commit, list } = mru(['a', 'b', 'c']);
        cycle.step();
        cycle.step();
        vi.advanceTimersByTime(PAUSE);
        expect(commit).toHaveBeenCalledTimes(1);
        expect(commit).toHaveBeenCalledWith('c');
        expect(cycle.active).toBe(false);
        expect(list()).toEqual(['c', 'a', 'b']);
    });

    it('a_pause_then_a_press_swaps_back', () => {
        const { cycle } = mru(['a', 'b', 'c']);
        expect(cycle.step()).toBe('b');
        vi.advanceTimersByTime(PAUSE);
        expect(cycle.step()).toBe('a');
        vi.advanceTimersByTime(PAUSE);
        expect(cycle.step()).toBe('b');
    });

    it('each_press_restarts_the_pause', () => {
        const { cycle, commit } = mru(['a', 'b', 'c']);
        cycle.step();
        vi.advanceTimersByTime(PAUSE - 1);
        cycle.step();
        vi.advanceTimersByTime(PAUSE - 1);
        expect(commit).not.toHaveBeenCalled();
        vi.advanceTimersByTime(1);
        expect(commit).toHaveBeenCalledWith('c');
    });

    it('settle_commits_immediately', () => {
        const { cycle, commit } = mru(['a', 'b', 'c']);
        cycle.step();
        cycle.settle();
        expect(commit).toHaveBeenCalledTimes(1);
        expect(commit).toHaveBeenCalledWith('b');
        vi.advanceTimersByTime(PAUSE);
        expect(commit).toHaveBeenCalledTimes(1);
    });

    it('settle_without_a_chain_does_nothing', () => {
        const { cycle, commit } = mru(['a', 'b', 'c']);
        cycle.settle();
        expect(commit).not.toHaveBeenCalled();
    });

    it('a_current_item_outside_the_ring_starts_from_the_head', () => {
        const { cycle } = mru(['a', 'b', 'c'], () => null);
        expect([cycle.step(), cycle.step(), cycle.step(), cycle.step()]).toEqual(['a', 'b', 'c', 'a']);
    });

    it('a_current_item_below_the_head_is_treated_as_the_head', () => {
        const { cycle } = mru(['a', 'b', 'c'], () => 'c');
        expect([cycle.step(), cycle.step(), cycle.step()]).toEqual(['a', 'b', 'c']);
    });

    it('the_walk_is_cut_to_the_limit_counting_the_current_item', () => {
        const { cycle } = mru(['a', 'b', 'c', 'd', 'e'], undefined, 3);
        expect([cycle.step(), cycle.step(), cycle.step(), cycle.step()]).toEqual(['b', 'c', 'a', 'b']);
    });

    it('the_limit_counts_from_a_current_item_below_the_head', () => {
        const { cycle } = mru(['a', 'b', 'c', 'd'], () => 'd', 2);
        expect([cycle.step(), cycle.step(), cycle.step()]).toEqual(['a', 'd', 'a']);
    });

    it('stepping_back_starts_from_the_far_end_and_wraps', () => {
        const { cycle } = mru(['a', 'b', 'c']);
        expect([cycle.step(-1), cycle.step(-1), cycle.step(-1), cycle.step(-1)]).toEqual(['c', 'b', 'a', 'c']);
    });

    it('stepping_back_undoes_an_overshoot_within_the_chain', () => {
        const { cycle, commit } = mru(['a', 'b', 'c', 'd']);
        expect([cycle.step(), cycle.step(), cycle.step(-1)]).toEqual(['b', 'c', 'b']);
        vi.advanceTimersByTime(PAUSE);
        expect(commit).toHaveBeenCalledOnce();
        expect(commit).toHaveBeenCalledWith('b');
    });

    it('stepping_back_respects_the_limit', () => {
        const { cycle } = mru(['a', 'b', 'c', 'd', 'e'], undefined, 3);
        expect(cycle.step(-1)).toBe('c');
    });

    it('stepping_back_with_nothing_current_starts_from_the_tail', () => {
        const { cycle } = mru(['a', 'b', 'c'], () => null);
        expect([cycle.step(-1), cycle.step(-1), cycle.step(-1)]).toEqual(['c', 'b', 'a']);
    });

    it('an_empty_ring_is_a_no_op', () => {
        const { cycle } = mru([], () => null);
        expect(cycle.canStep()).toBe(false);
        expect(cycle.step()).toBeNull();
        expect(cycle.active).toBe(false);
    });

    it('a_ring_holding_only_the_current_item_is_a_no_op', () => {
        const { cycle } = mru(['a']);
        expect(cycle.canStep()).toBe(false);
        expect(cycle.step()).toBeNull();
        expect(cycle.active).toBe(false);
    });

    it('a_one_entry_ring_with_nothing_current_lands_once', () => {
        const { cycle, commit } = mru(['x'], () => null);
        expect(cycle.canStep()).toBe(true);
        expect(cycle.step()).toBe('x');
        expect(cycle.step()).toBeNull();
        expect(cycle.active).toBe(true);
        vi.advanceTimersByTime(PAUSE);
        expect(commit).toHaveBeenCalledTimes(1);
        expect(commit).toHaveBeenCalledWith('x');
    });
});
