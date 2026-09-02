/**
 * Autosave scheduler. On a configurable interval it snapshots the active
 * tab (when dirty and idle) to OPFS for crash recovery, and it snapshots a
 * tab when you switch away from it so every open document is covered. The
 * snapshot bytes come from `produceDarklyBytes`, which keeps the tab's
 * render loop alive (via `onSaveResult`) until the readback lands, so even
 * a backgrounded tab completes without the artist looking at it.
 *
 * Snapshots reuse the exact `.darkly` save pipeline and are marked
 * `'snapshot'` so they leave the document's dirty flag set (nothing
 * reached the artist's file). See `storage/recovery.ts` for the store and
 * `state/recoverySession.ts` for crash detection.
 */
import { config } from '../config/store.svelte';
import { shell } from '../multi_tab/shell.svelte';
import { produceDarklyBytes } from '../storage/saveDocument';
import { writeSnapshot } from '../storage/recovery';
import { sessionId } from './recoverySession';
import type { DarklyInstance } from './app.svelte';

/** Skip a switch-away snapshot if the tab was snapshotted this recently:
 *  rapid tab-flipping shouldn't trigger a composite hitch each time. */
const SWITCH_DEBOUNCE_MS = 5_000;

/** Pure eligibility check, extracted for unit testing. A snapshot runs
 *  only when the tab has unsaved work, the artist isn't mid-stroke, no
 *  snapshot is already in flight for it, and (for switch-away) it wasn't
 *  snapshotted within the debounce window. */
export function snapshotEligible(opts: {
    isDirty: boolean;
    idle: boolean;
    inFlight: boolean;
    lastSnapshotAt: number | undefined;
    now: number;
    debounceMs: number;
}): boolean {
    if (!opts.isDirty) return false;
    if (!opts.idle) return false;
    if (opts.inFlight) return false;
    if (opts.debounceMs > 0 && opts.now - (opts.lastSnapshotAt ?? 0) < opts.debounceMs) {
        return false;
    }
    return true;
}

class AutosaveScheduler {
    private timer: ReturnType<typeof setInterval> | null = null;
    /** recoveryIds with a snapshot currently being produced: guards the
     *  single per-engine save slot against overlapping autosave ticks. */
    private inFlight = new Set<string>();
    /** recoveryId → last successful snapshot time (ms), for debouncing. */
    private lastSnapshotAt = new Map<string, number>();
    private stopWatch: (() => void) | null = null;
    private started = false;

    /** Wire the scheduler to config + tab-switch events. Idempotent. */
    start(): void {
        if (this.started) return;
        this.started = true;
        config.onChange(() => this.reconfigure());
        this.reconfigure();

        // Snapshot a tab when focus leaves it (it's still alive and its
        // snapshot drives to completion on its own render loop). Watching
        // `shell.active` reactively keeps the shell ignorant of autosave.
        this.stopWatch = $effect.root(() => {
            let prev: DarklyInstance | null = null;
            $effect(() => {
                const cur = shell.active;
                // Only snapshot a tab we switched AWAY from while it's still
                // open; a *closed* tab's engine is freed (and closeGuard
                // already cleared its snapshot).
                if (prev && prev !== cur && shell.instances.includes(prev)) {
                    void this.snapshot(prev, SWITCH_DEBOUNCE_MS);
                }
                prev = cur;
            });
        });
    }

    /** Tear down (tests / HMR). */
    stop(): void {
        if (this.timer !== null) clearInterval(this.timer);
        this.timer = null;
        this.stopWatch?.();
        this.stopWatch = null;
        this.started = false;
    }

    /** Re-read config and (re)arm the interval timer. */
    private reconfigure(): void {
        if (this.timer !== null) clearInterval(this.timer);
        this.timer = null;
        if (!(config.get('autosave.enabled') as boolean)) return;
        const seconds = Math.max(30, config.get('autosave.intervalSeconds') as number);
        this.timer = setInterval(() => this.tick(), seconds * 1000);
    }

    /** Interval tick: snapshot the focused tab if it has unsaved work. */
    private tick(): void {
        const inst = shell.active;
        if (inst) void this.snapshot(inst);
    }

    /**
     * Snapshot `inst` to OPFS if eligible. No-op when the tab is clean,
     * mid-stroke, already snapshotting, or (with `debounceMs`) snapshotted
     * very recently. Swallows `SaveError::InProgress` (a manual save holds
     * the slot) and transient failures; the next tick retries.
     */
    async snapshot(inst: DarklyInstance, debounceMs = 0): Promise<void> {
        const engine = inst.engine;
        if (!engine) return;
        const dirty = await engine.api.isDirty();
        const eligible = snapshotEligible({
            isDirty: dirty,
            idle: inst.idleForSnapshot,
            inFlight: this.inFlight.has(inst.recoveryId),
            lastSnapshotAt: this.lastSnapshotAt.get(inst.recoveryId),
            now: Date.now(),
            debounceMs,
        });
        if (!eligible) return;

        this.inFlight.add(inst.recoveryId);
        try {
            const bytes = await produceDarklyBytes(inst, 'snapshot');
            await writeSnapshot(sessionId, inst.recoveryId, bytes);
            this.lastSnapshotAt.set(inst.recoveryId, Date.now());
        } catch {
            // InProgress / transient: skip, the next tick retries.
        } finally {
            this.inFlight.delete(inst.recoveryId);
        }
    }
}

export const autosave = new AutosaveScheduler();

if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
