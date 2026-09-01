import { describe, it, expect } from 'vitest';
import { snapshotEligible } from '../autosave.svelte';

const base = {
    isDirty: true,
    idle: true,
    inFlight: false,
    lastSnapshotAt: undefined as number | undefined,
    now: 10_000,
    debounceMs: 0,
};

describe('snapshotEligible', () => {
    it('snapshots a dirty, idle, not-in-flight tab', () => {
        expect(snapshotEligible(base)).toBe(true);
    });

    it('skips a clean tab', () => {
        expect(snapshotEligible({ ...base, isDirty: false })).toBe(false);
    });

    it('skips while the artist is mid-stroke (not idle)', () => {
        expect(snapshotEligible({ ...base, idle: false })).toBe(false);
    });

    it('skips when a snapshot is already in flight (single save slot)', () => {
        expect(snapshotEligible({ ...base, inFlight: true })).toBe(false);
    });

    it('debounces a switch-away snapshot taken very recently', () => {
        expect(
            snapshotEligible({ ...base, debounceMs: 5_000, lastSnapshotAt: 8_000, now: 10_000 }),
        ).toBe(false);
        // Outside the window → eligible again.
        expect(
            snapshotEligible({ ...base, debounceMs: 5_000, lastSnapshotAt: 3_000, now: 10_000 }),
        ).toBe(true);
    });
});
