import { describe, it, expect, vi, beforeEach } from 'vitest';

import {
    SessionEngine,
    ToolSessionCancelled,
    beginToolSession,
    killToolSession,
    toolEngine,
    runHook,
} from '../tool_session';

/** A deferred promise so a `send` can be parked, then resolved after a kill. */
function deferred<T>() {
    let resolve!: (v: T) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

beforeEach(() => {
    killToolSession();
});

describe('SessionEngine.send', () => {
    it('resolves normally while the session is alive', async () => {
        const inner = { send: vi.fn(() => Promise.resolve({ value: 7 })), post: vi.fn() };
        const session = new SessionEngine(inner as never);
        await expect(session.send('has_floating')).resolves.toEqual({ value: 7 });
        expect(inner.send).toHaveBeenCalledWith('has_floating', undefined, undefined);
    });

    it('rejects an in-flight send with ToolSessionCancelled once killed', async () => {
        const gate = deferred<{ value: boolean }>();
        const inner = { send: vi.fn(() => gate.promise), post: vi.fn() };
        const session = new SessionEngine(inner as never);

        const p = session.send('has_floating');
        // The world changes while the request is parked.
        session.kill();
        gate.resolve({ value: true });

        await expect(p).rejects.toBeInstanceOf(ToolSessionCancelled);
    });

    it('drops a post issued after kill', () => {
        const inner = { send: vi.fn(), post: vi.fn() };
        const session = new SessionEngine(inner as never);

        session.post('clear_overlay');
        expect(inner.post).toHaveBeenCalledTimes(1);

        session.kill();
        session.post('clear_overlay');
        // Still 1 — the post-kill call was dropped.
        expect(inner.post).toHaveBeenCalledTimes(1);
    });
});

describe('session registry', () => {
    it('beginToolSession installs the live session and kills the prior one', async () => {
        const inner = { send: vi.fn(() => Promise.resolve('ok')), post: vi.fn() };

        const first = beginToolSession(inner as never);
        expect(toolEngine()).toBe(first);

        const parked = first.send('has_floating'); // in flight on the first session
        const second = beginToolSession(inner as never);
        expect(toolEngine()).toBe(second);

        // Beginning the second session killed the first, so its parked op rejects.
        await expect(parked).rejects.toBeInstanceOf(ToolSessionCancelled);
        // The new session is alive.
        await expect(second.send('has_floating')).resolves.toBe('ok');
    });

    it('killToolSession leaves no live session', () => {
        beginToolSession({ send: vi.fn(), post: vi.fn() } as never);
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
