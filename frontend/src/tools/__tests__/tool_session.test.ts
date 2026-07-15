import { describe, it, expect, vi, beforeEach } from 'vitest';

import {
    SessionEngine,
    ToolSessionCancelled,
    beginToolSession,
    killToolSession,
    toolEngine,
    runHook,
} from '../tool_session';

/** A deferred promise so a request can be parked, then resolved after a kill. */
function deferred<T>() {
    let resolve!: (v: T) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

/** A stand-in for the real `Engine`: only its `transport` is needed, since a
 *  `SessionEngine` layers cancellation over that and builds its own typed api. */
function fakeEngine(request: () => Promise<unknown>, postFF: () => void = () => {}) {
    return { transport: { request: vi.fn(request), postFF: vi.fn(postFF) } };
}

beforeEach(() => {
    killToolSession();
});

describe('SessionEngine.api', () => {
    it('resolves normally while the session is alive', async () => {
        const inner = fakeEngine(() => Promise.resolve(true));
        const session = new SessionEngine(inner as never);
        await expect(session.api.hasFloating()).resolves.toBe(true);
        expect(inner.transport.request).toHaveBeenCalledWith('has_floating', undefined, undefined);
    });

    it('rejects an in-flight request with ToolSessionCancelled once killed', async () => {
        const gate = deferred<boolean>();
        const inner = fakeEngine(() => gate.promise);
        const session = new SessionEngine(inner as never);

        const p = session.api.hasFloating();
        // The world changes while the request is parked.
        session.kill();
        gate.resolve(true);

        await expect(p).rejects.toBeInstanceOf(ToolSessionCancelled);
    });

    it('drops a fire-and-forget request issued after kill', () => {
        const inner = fakeEngine(() => Promise.resolve(null));
        const session = new SessionEngine(inner as never);

        session.api.clearOverlay();
        expect(inner.transport.postFF).toHaveBeenCalledTimes(1);

        session.kill();
        session.api.clearOverlay();
        // Still 1 — the post-kill call was dropped.
        expect(inner.transport.postFF).toHaveBeenCalledTimes(1);
    });
});

describe('session registry', () => {
    it('beginToolSession installs the live session and kills the prior one', async () => {
        const first = beginToolSession(fakeEngine(() => Promise.resolve(true)) as never);
        expect(toolEngine()).toBe(first);

        const parked = first.api.hasFloating(); // in flight on the first session
        const second = beginToolSession(fakeEngine(() => Promise.resolve(true)) as never);
        expect(toolEngine()).toBe(second);

        // Beginning the second session killed the first, so its parked op rejects.
        await expect(parked).rejects.toBeInstanceOf(ToolSessionCancelled);
        // The new session is alive.
        await expect(second.api.hasFloating()).resolves.toBe(true);
    });

    it('killToolSession leaves no live session', () => {
        beginToolSession(fakeEngine(() => Promise.resolve(null)) as never);
        killToolSession();
        expect(toolEngine()).toBeNull();
    });
});

describe('runHook', () => {
    it('swallows ToolSessionCancelled', async () => {
        await expect(runHook(Promise.reject(new ToolSessionCancelled()))).resolves.toBeUndefined();
    });

    it('re-throws any other error', async () => {
        const boom = new Error('boom');
        await expect(runHook(Promise.reject(boom))).rejects.toBe(boom);
    });

    it('passes through a resolved (or sync) hook', async () => {
        await expect(runHook(undefined)).resolves.toBeUndefined();
        await expect(runHook(Promise.resolve(42))).resolves.toBeUndefined();
    });
});
