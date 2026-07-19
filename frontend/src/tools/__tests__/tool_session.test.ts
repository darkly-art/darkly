import { describe, it, expect, vi, afterEach } from 'vitest';

import {
    SessionEngine,
    ToolSessionCancelled,
    runHook,
    planToolTransition,
    type ToolTransitionState,
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

    it('a fresh session over the same engine is alive while the prior one is dead', async () => {
        const inner = fakeEngine(() => Promise.resolve(true));
        const first = new SessionEngine(inner as never);
        const parked = first.api.hasFloating();
        first.kill();
        const second = new SessionEngine(inner as never);
        await expect(parked).rejects.toBeInstanceOf(ToolSessionCancelled);
        await expect(second.api.hasFloating()).resolves.toBe(true);
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

describe('planToolTransition', () => {
    const base: ToolTransitionState = {
        hasEngine: true,
        hasCanvas: true,
        toolId: 'brush',
        layerId: 1,
        reactivations: 0,
    };
    const s = (over: Partial<ToolTransitionState>): ToolTransitionState => ({ ...base, ...over });

    it('kills when there is no engine or no canvas', () => {
        expect(planToolTransition(base, s({ hasEngine: false })).kill).toBe(true);
        expect(planToolTransition(base, s({ hasCanvas: false })).kill).toBe(true);
        // Kill short-circuits everything else.
        const p = planToolTransition(base, s({ hasEngine: false }));
        expect(p).toMatchObject({ deactivate: false, rebind: false, activate: false, dismiss: false });
    });

    it('a tool change deactivates the old tool, rebinds, and activates', () => {
        expect(planToolTransition(base, s({ toolId: 'transform' }))).toEqual({
            kill: false, deactivate: true, rebind: true, activate: true, dismiss: false,
        });
    });

    it('a reactivation request rebinds + activates, never deactivates', () => {
        expect(planToolTransition(base, s({ reactivations: 1 }))).toEqual({
            kill: false, deactivate: false, rebind: true, activate: true, dismiss: false,
        });
    });

    it('a layer change rebinds + dismisses', () => {
        expect(planToolTransition(base, s({ layerId: 2 }))).toEqual({
            kill: false, deactivate: false, rebind: true, activate: false, dismiss: true,
        });
    });

    it('no change is a no-op', () => {
        expect(planToolTransition(base, s({}))).toEqual({
            kill: false, deactivate: false, rebind: false, activate: false, dismiss: false,
        });
    });

    it('tool + layer in one flush is a single rebind with no dismiss (tool wins)', () => {
        const p = planToolTransition(base, s({ toolId: 'transform', layerId: 2 }));
        expect(p).toEqual({ kill: false, deactivate: true, rebind: true, activate: true, dismiss: false });
    });

    it('reactivate + layer in one flush rebinds+activates, no deactivate/dismiss (reactivate wins)', () => {
        const p = planToolTransition(base, s({ reactivations: 1, layerId: 2 }));
        expect(p).toEqual({ kill: false, deactivate: false, rebind: true, activate: true, dismiss: false });
    });

    it('engine-before-canvas: kill first, then activate once the canvas arrives', () => {
        // Engine present but canvas not → kill.
        const kill = planToolTransition(base, s({ hasCanvas: false }));
        expect(kill.kill).toBe(true);
        // The effect blanks the tool id after a kill; the canvas arriving then
        // reads as a tool change → activate.
        const afterKill = s({ toolId: '' });
        const arrive = planToolTransition(afterKill, s({}));
        expect(arrive).toMatchObject({ kill: false, deactivate: true, rebind: true, activate: true });
    });
});

describe('setupToolSessionRejectionGuard', () => {
    afterEach(() => {
        vi.unstubAllGlobals();
        vi.resetModules();
    });

    it('preventDefaults ToolSessionCancelled, ignores other errors, installs once', async () => {
        const handlers: Array<(e: { reason: unknown; preventDefault: () => void }) => void> = [];
        const fakeWindow = {
            addEventListener: vi.fn((type: string, h: (e: never) => void) => {
                if (type === 'unhandledrejection') handlers.push(h as never);
            }),
        };
        vi.stubGlobal('window', fakeWindow);

        // Fresh module so `guardWired` starts false.
        const mod = await import('../tool_session');
        mod.setupToolSessionRejectionGuard();
        mod.setupToolSessionRejectionGuard(); // idempotent
        expect(fakeWindow.addEventListener).toHaveBeenCalledTimes(1);

        const handler = handlers[0];
        const cancelled = { reason: new mod.ToolSessionCancelled(), preventDefault: vi.fn() };
        handler(cancelled);
        expect(cancelled.preventDefault).toHaveBeenCalledTimes(1);

        const other = { reason: new Error('real'), preventDefault: vi.fn() };
        handler(other);
        expect(other.preventDefault).not.toHaveBeenCalled();
    });
});
