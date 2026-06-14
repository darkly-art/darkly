import { describe, it, expect } from 'vitest';
import {
    readRegistry,
    writeRegistry,
    classifySessions,
    partitionSnapshots,
    claimAndRegister,
    unregisterSession,
    STALE_MS,
    type KeyValueStore,
} from '../recoverySession';
import type { RecoveryEntry } from '../../storage/recovery';

/** In-memory localStorage stand-in. */
function fakeLS(initial: Record<string, string> = {}): KeyValueStore {
    const map = new Map(Object.entries(initial));
    return {
        getItem: (k) => map.get(k) ?? null,
        setItem: (k, v) => { map.set(k, v); },
    };
}

const NOW = 1_000_000;

describe('classifySessions', () => {
    it('marks stale prior sessions crashed and fresh ones live, excluding self', () => {
        const reg = {
            me: NOW,
            crashed: NOW - STALE_MS - 1,
            live: NOW - 1_000,
        };
        const { crashed, live } = classifySessions(reg, 'me', NOW);
        expect([...crashed]).toEqual(['crashed']);
        expect([...live]).toEqual(['live']);
        expect(crashed.has('me')).toBe(false);
        expect(live.has('me')).toBe(false);
    });
});

describe('partitionSnapshots', () => {
    const entry = (sessionId: string, recoveryId: string): RecoveryEntry => ({
        sessionId,
        recoveryId,
        name: `${sessionId}/${recoveryId}`,
    });

    it('offers crashed-owned snapshots, ignores live/self, GCs the rest', () => {
        const entries = [
            entry('crashed', 'a'),
            entry('live', 'b'),
            entry('self', 'c'),
            entry('gone', 'd'),
        ];
        const { offered, orphans } = partitionSnapshots(
            entries,
            new Set(['crashed']),
            new Set(['live']),
            'self',
        );
        expect(offered.map((e) => e.recoveryId)).toEqual(['a']);
        expect(orphans.map((e) => e.recoveryId)).toEqual(['d']);
    });
});

describe('session registry lifecycle', () => {
    it('round-trips and treats corrupt JSON as empty', () => {
        const ls = fakeLS();
        writeRegistry(ls, { a: 1, b: 2 });
        expect(readRegistry(ls)).toEqual({ a: 1, b: 2 });

        const corrupt = fakeLS({ 'darkly.recovery.sessions': '{not json' });
        expect(readRegistry(corrupt)).toEqual({});
    });

    it('claimAndRegister adds self and drops crashed sessions', () => {
        const ls = fakeLS();
        writeRegistry(ls, { crashed: NOW - STALE_MS - 1, live: NOW - 1 });
        claimAndRegister(ls, 'me', NOW, new Set(['crashed']));

        const reg = readRegistry(ls);
        expect(reg.me).toBe(NOW);
        expect(reg.live).toBeDefined();
        expect(reg.crashed).toBeUndefined();
    });

    it('unregisterSession removes self (clean exit ⇒ not crashed next boot)', () => {
        const ls = fakeLS();
        writeRegistry(ls, { me: NOW, other: NOW });
        unregisterSession(ls, 'me');

        const reg = readRegistry(ls);
        expect(reg.me).toBeUndefined();
        expect(reg.other).toBe(NOW);
        // A subsequent boot sees no 'me' entry → no crash prompt for it.
        const { crashed } = classifySessions(reg, 'boot2', NOW + STALE_MS + 10);
        expect(crashed.has('me')).toBe(false);
    });
});
