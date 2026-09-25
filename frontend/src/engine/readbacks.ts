/**
 * One-shot engine readbacks, as a queue.
 *
 * Several engine operations (copy, export, `.darkly` save) hand their result
 * back through the same two-step shape: a `start_*` request kicks the work, and
 * a `poll_*_result` request returns `null` until the GPU readback lands. Nothing
 * may block on the mapping (see the No Blocking GPU Readbacks rule in
 * CLAUDE.md), so the frame loop is what drives the polling.
 *
 * The queue turns that shape into a promise. A caller registers the poll it
 * wants driven, the loop calls {@link ReadbackQueue.poll} once per frame, and
 * the first non-null value resolves the caller's promise and drops the entry.
 * Adding another readback to the protocol needs no change here and no new
 * branch in the loop.
 *
 * Scope: **one-shot** readbacks only, meaning ones that produce a single
 * terminal value and are then done. Continuous per-frame streams (brush
 * previews, recording frames) have no terminal value and stay their own named
 * calls in the loop; folding them in here would never settle.
 *
 * The queue holds no engine reference: the `poll` closure carries the whole
 * contract, which also makes it directly testable with a plain function.
 */

/**
 * Which engine-side result slot a readback is waiting on.
 *
 * The engine holds exactly one pending result per kind and `poll_*_result`
 * takes it, so two waiters on one slot cannot both be served: the second start
 * overwrites the first's result and one promise would wait forever, keeping the
 * frame loop scheduling with it. Naming the slot is what lets the queue enforce
 * that instead of discovering it as a hang.
 */
export type ReadbackSlot = 'copy' | 'export' | 'save';

/** One outstanding readback: the poll that drives it, and the promise it settles. */
interface Readback<T> {
    poll: () => Promise<T | null>;
    resolve: (value: T) => void;
    reject: (reason: Error) => void;
    /** A poll is already awaiting the transport; don't stack a second one on it. */
    inFlight: boolean;
    /** Settled, so a poll that lands after the entry was dropped is ignored. */
    done: boolean;
}

export class ReadbackQueue {
    #entries = new Map<ReadbackSlot, Readback<any>>();

    /** Drive `poll` once per frame until it returns non-null, then resolve with
     *  that value.
     *
     *  Rejects if the poll request fails, if {@link abort} is called first (the
     *  owning tab closed), or if another readback claims the same `slot` before
     *  this one lands: the newer request is the one the artist asked for, and
     *  the engine has only the one result to give. */
    awaitResult<T>(slot: ReadbackSlot, poll: () => Promise<T | null>): Promise<T> {
        this.#drop(slot, `readback superseded: another ${slot} started before this one landed`);
        return new Promise<T>((resolve, reject) => {
            this.#entries.set(slot, { poll, resolve, reject, inFlight: false, done: false });
        });
    }

    /** Issue one poll per outstanding readback. Called from the frame loop.
     *  Entries whose previous poll has not come back yet are skipped, so a slow
     *  readback gets one in-flight request rather than one per frame. */
    poll(): void {
        for (const [slot, entry] of this.#entries) {
            if (entry.inFlight) continue;
            entry.inFlight = true;
            entry.poll().then(
                (result) => {
                    entry.inFlight = false;
                    if (entry.done || result == null) return;
                    this.#settle(slot, entry);
                    entry.resolve(result);
                },
                (e: unknown) => {
                    entry.inFlight = false;
                    if (entry.done) return;
                    this.#settle(slot, entry);
                    entry.reject(e instanceof Error ? e : new Error(String(e)));
                },
            );
        }
    }

    /** Outstanding readbacks. The frame loop keeps scheduling while non-zero,
     *  so a save on a backgrounded tab still runs to completion. */
    get pending(): number {
        return this.#entries.size;
    }

    /** Reject everything outstanding. Called from `dispose()`: once the handle
     *  is freed no poll can ever land, so a waiting save or export must fail
     *  rather than hang forever. */
    abort(): void {
        for (const slot of [...this.#entries.keys()]) {
            this.#drop(slot, 'readback aborted: the instance was disposed');
        }
    }

    /** Settle and forget the entry holding `slot`, if it is still the live one. */
    #settle(slot: ReadbackSlot, entry: Readback<any>): void {
        entry.done = true;
        if (this.#entries.get(slot) === entry) this.#entries.delete(slot);
    }

    #drop(slot: ReadbackSlot, reason: string): void {
        const entry = this.#entries.get(slot);
        if (!entry) return;
        this.#entries.delete(slot);
        if (entry.done) return;
        entry.done = true;
        entry.reject(new Error(reason));
    }
}
