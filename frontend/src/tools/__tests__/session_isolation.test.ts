import { describe, it, expect, vi } from 'vitest';
import { DarklyInstance } from '../../state/app.svelte';
import { ToolSessionCancelled, runHook } from '../tool_session';
// Populate the tool registry (descriptors) so `inst.tool(id)` can construct.
import '../index';

// Incident regression: painting died with an uncaught `ToolSessionCancelled` in
// multi-tab: a background tab's engine finished async init and (under the old
// module-global session) its `CanvasView` effect stole the focused tab's
// session, cancelling the focused tab's in-flight tool op. With the session
// owned per `DarklyInstance`, a background instance beginning / rebinding its
// own session cannot touch the focused instance's session at all.

/** A deferred promise so a request can be parked, then resolved later. */
function deferred<T>() {
    let resolve!: (v: T) => void;
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
}

/** A stand-in for the real `Engine`: only `transport` is needed for a
 *  `SessionEngine` to layer cancellation over it. */
function fakeEngine(request: () => Promise<unknown>) {
    return { transport: { request: vi.fn(request), postFF: vi.fn() } };
}

describe('per-instance tool-session isolation', () => {
    it("a background instance rebinding its own session leaves the focused instance's in-flight op alive", async () => {
        const gateA = deferred<boolean>();

        // Focused instance A: live session with a parked request.
        const a = new DarklyInstance();
        a.requestFrame = vi.fn();
        a.engine = fakeEngine(() => gateA.promise) as never;
        a.beginToolSession();
        const sessionA = a.session;
        const parked = a.session!.api.hasFloating(); // in flight on A

        // Background instance B finishes async init: its transition effect
        // begins ITS OWN session, and a subsequent active-layer change rebinds
        // it again. Neither touches A.
        const b = new DarklyInstance();
        b.requestFrame = vi.fn();
        b.engine = fakeEngine(() => Promise.resolve(true)) as never;
        b.beginToolSession();
        b.beginToolSession(); // simulate B's layer-change rebind

        expect(a.session).toBe(sessionA); // untouched
        expect(a.session).not.toBe(b.session);

        // A's parked op resolves on A's still-live session; no cancellation.
        gateA.resolve(true);
        await expect(parked).resolves.toBe(true);
    });

    it("rebinding an instance's own session cancels only that instance's parked op", async () => {
        const gate = deferred<boolean>();
        const inst = new DarklyInstance();
        inst.requestFrame = vi.fn();
        inst.engine = fakeEngine(() => gate.promise) as never;
        inst.beginToolSession();
        const parked = inst.session!.api.hasFloating();

        // A tool/layer change on THIS instance rebinds its session (as the
        // transition effect does), killing the prior one.
        inst.beginToolSession();
        gate.resolve(true);

        await expect(parked).rejects.toBeInstanceOf(ToolSessionCancelled);
    });

    it('killToolSession severs the session; a parked op rejects on resume', async () => {
        const gate = deferred<boolean>();
        const inst = new DarklyInstance();
        inst.requestFrame = vi.fn();
        inst.engine = fakeEngine(() => gate.promise) as never;
        inst.beginToolSession();
        const parked = inst.session!.api.hasFloating();

        inst.killToolSession(); // tab close
        expect(inst.session).toBeNull();
        gate.resolve(true);

        await expect(parked).rejects.toBeInstanceOf(ToolSessionCancelled);
    });

    it('a runHook-wrapped void spawn swallows the cancellation a bare spawn would leak', async () => {
        // `process` isn't in svelte-check's browser lib; reach it off globalThis.
        const proc = (globalThis as unknown as {
            process: {
                on(e: string, cb: (x: unknown) => void): void;
                off(e: string, cb: (x: unknown) => void): void;
            };
        }).process;
        const unhandled: unknown[] = [];
        const onUnhandled = (e: unknown) => unhandled.push(e);
        proc.on('unhandledRejection', onUnhandled);
        try {
            // Bare spawn (the pre-fix shape): the op resolves on a dead session,
            // so its ToolSessionCancelled surfaces as an unhandled rejection.
            const bareGate = deferred<boolean>();
            const inst = new DarklyInstance();
            inst.requestFrame = vi.fn();
            inst.engine = fakeEngine(() => bareGate.promise) as never;
            inst.beginToolSession();
            void inst.session!.api.hasFloating(); // NOT runHook-wrapped
            inst.beginToolSession(); // kills the session mid-await
            bareGate.resolve(true);
            await new Promise((r) => setTimeout(r, 0));
            expect(unhandled).toHaveLength(1);
            expect(unhandled[0]).toBeInstanceOf(ToolSessionCancelled);

            // The dispatcher shape: the same op wrapped in runHook settles
            // cleanly; no additional unhandled rejection.
            const wrappedGate = deferred<boolean>();
            const inst2 = new DarklyInstance();
            inst2.requestFrame = vi.fn();
            inst2.engine = fakeEngine(() => wrappedGate.promise) as never;
            inst2.beginToolSession();
            void runHook(inst2.session!.api.hasFloating());
            inst2.beginToolSession();
            wrappedGate.resolve(true);
            await new Promise((r) => setTimeout(r, 0));
            expect(unhandled).toHaveLength(1); // still just the bare one
        } finally {
            proc.off('unhandledRejection', onUnhandled);
        }
    });

    it('lazily constructs one tool per id, bound to this instance', () => {
        const inst = new DarklyInstance();
        const brush1 = inst.tool('brush');
        const brush2 = inst.tool('brush');
        expect(brush1).toBeDefined();
        expect(brush1).toBe(brush2); // cached, not rebuilt
        expect(inst.tool('does_not_exist')).toBeUndefined();

        // A second instance gets its OWN tool object.
        const other = new DarklyInstance();
        expect(other.tool('brush')).not.toBe(brush1);
    });
});
