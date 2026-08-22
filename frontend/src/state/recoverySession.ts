/**
 * Browser-session registry + crash detection for autosave recovery.
 *
 * Each page load is a "session" with a UUID. While running, a session
 * keeps a heartbeat (a timestamp in `localStorage`) refreshed on its own
 * lightweight timer — independent of the autosave cadence, so liveness is
 * still tracked when autosave is off or its interval is long. On a clean
 * exit (`pagehide`) the session removes itself synchronously.
 *
 * At the next boot:
 *  - a prior session **still in the registry** with a **stale** heartbeat
 *    never fired `pagehide` ⇒ it crashed ⇒ its snapshots are offered;
 *  - a prior session with a **fresh** heartbeat is a live concurrent
 *    browser tab ⇒ leave its snapshots alone;
 *  - a snapshot owned by neither a crashed nor a live session is an
 *    orphan of a cleanly-exited session ⇒ garbage-collect it (no prompt
 *    on a normal reload/close).
 *
 * The pure helpers (`classifySessions`, `partitionSnapshots`) carry the
 * logic and are unit-tested with injected clock/registry; the thin wiring
 * (`initRecoverySession`) attaches the timer and `pagehide` handler.
 */
import { listSnapshots, removeSnapshot, type RecoveryEntry } from '../storage/recovery';
import { storage as defaultStorage, type DarklyStorage } from '../storage';
import { newId } from '../lib/id';

const REGISTRY_KEY = 'darkly.recovery.sessions';
/** Heartbeat cadence — how often a live session refreshes its timestamp. */
export const HEARTBEAT_MS = 10_000;
/** A heartbeat older than this marks the session crashed (≈3 missed beats). */
export const STALE_MS = 30_000;

/** Minimal `localStorage` surface, injectable for tests. */
export interface KeyValueStore {
    getItem(key: string): string | null;
    setItem(key: string, value: string): void;
}

type Registry = Record<string, number>;

const genId = () => newId('session');

export function readRegistry(ls: KeyValueStore): Registry {
    const raw = ls.getItem(REGISTRY_KEY);
    if (!raw) return {};
    try {
        const parsed = JSON.parse(raw) as unknown;
        if (parsed && typeof parsed === 'object') return parsed as Registry;
    } catch {
        /* corrupt → treat as empty */
    }
    return {};
}

export function writeRegistry(ls: KeyValueStore, reg: Registry): void {
    ls.setItem(REGISTRY_KEY, JSON.stringify(reg));
}

/** Split the registry's prior sessions (excluding `selfId`) into crashed
 *  (stale heartbeat) and live (fresh heartbeat) sets. */
export function classifySessions(
    reg: Registry,
    selfId: string,
    now: number,
): { crashed: Set<string>; live: Set<string> } {
    const crashed = new Set<string>();
    const live = new Set<string>();
    for (const [id, lastBeat] of Object.entries(reg)) {
        if (id === selfId) continue;
        if (now - lastBeat > STALE_MS) crashed.add(id);
        else live.add(id);
    }
    return { crashed, live };
}

/** Partition on-disk snapshots by owning-session liveness. Snapshots from
 *  crashed sessions are `offered`; snapshots owned by neither a crashed
 *  nor a live session (nor self) are `orphans` to be GC'd. */
export function partitionSnapshots(
    entries: RecoveryEntry[],
    crashed: Set<string>,
    live: Set<string>,
    selfId: string,
): { offered: RecoveryEntry[]; orphans: RecoveryEntry[] } {
    const offered: RecoveryEntry[] = [];
    const orphans: RecoveryEntry[] = [];
    for (const e of entries) {
        if (crashed.has(e.sessionId)) offered.push(e);
        else if (live.has(e.sessionId) || e.sessionId === selfId) continue;
        else orphans.push(e);
    }
    return { offered, orphans };
}

/** Register `selfId` with a fresh heartbeat and drop already-claimed
 *  crashed sessions so they aren't re-offered next boot. Pure registry
 *  mutation — testable without timers. */
export function claimAndRegister(
    ls: KeyValueStore,
    selfId: string,
    now: number,
    crashed: Set<string>,
): void {
    const reg = readRegistry(ls);
    const next: Registry = {};
    for (const [id, beat] of Object.entries(reg)) {
        if (!crashed.has(id)) next[id] = beat;
    }
    next[selfId] = now;
    writeRegistry(ls, next);
}

/** Remove `selfId` from the registry (clean-exit handler). */
export function unregisterSession(ls: KeyValueStore, selfId: string): void {
    const reg = readRegistry(ls);
    if (selfId in reg) {
        delete reg[selfId];
        writeRegistry(ls, reg);
    }
}

/** This page load's session id. Stable for the lifetime of the tab. */
export const sessionId: string = genId();

/**
 * Boot the recovery session: classify prior sessions, register self, and
 * start the heartbeat + clean-exit handler. Returns the crashed/live sets
 * computed *before* self was registered, for the recovery scan.
 */
export function initRecoverySession(
    ls: KeyValueStore = globalThis.localStorage,
    now: () => number = Date.now,
): { crashed: Set<string>; live: Set<string> } {
    const reg = readRegistry(ls);
    const classes = classifySessions(reg, sessionId, now());

    // Register self and drop already-claimed crashed sessions so they
    // aren't re-offered on a subsequent boot.
    claimAndRegister(ls, sessionId, now(), classes.crashed);

    const beat = () => {
        const r = readRegistry(ls);
        r[sessionId] = now();
        writeRegistry(ls, r);
    };
    if (typeof setInterval === 'function') setInterval(beat, HEARTBEAT_MS);

    // Clean exit: remove self synchronously so a normal reload/close does
    // not look like a crash next time.
    const unregister = () => unregisterSession(ls, sessionId);
    if (typeof addEventListener === 'function') {
        addEventListener('pagehide', unregister);
        addEventListener('beforeunload', unregister);
    }

    return classes;
}

/**
 * List recoverable snapshots (from crashed sessions) and garbage-collect
 * orphans (snapshots from cleanly-exited sessions). `crashed`/`live` come
 * from {@link initRecoverySession}.
 */
export async function collectRecovery(
    crashed: Set<string>,
    live: Set<string>,
    storage: DarklyStorage = defaultStorage,
): Promise<RecoveryEntry[]> {
    const entries = await listSnapshots(storage);
    const { offered, orphans } = partitionSnapshots(entries, crashed, live, sessionId);
    for (const o of orphans) {
        await removeSnapshot(o.sessionId, o.recoveryId, storage);
    }
    return offered;
}
