import { describe, it, expect, vi, beforeEach } from 'vitest';

// Drives the transform tool's async hooks against a mocked engine + gizmo to
// prove the tool-session mechanism (tool_session.ts) makes "resume into a
// changed world" unrepresentable: a parked engine read routed through a session
// that dies mid-await rejects with ToolSessionCancelled, unwinding the hook
// before it can dereference a nulled gizmo or drive an op on the wrong layer.
const { engine, fakeApp, gizmo, TransformGizmoMock, deferrals } = vi.hoisted(() => {
    const deferrals: Record<string, { resolve: (v: unknown) => void }> = {};
    const engine = {
        // Each kind may be answered eagerly (Promise.resolve) or parked via a
        // per-kind deferral the test resolves by name.
        send: vi.fn((kind: string) => {
            if (kind === 'layer_transform_capability' && deferrals.cap)
                return new Promise((r) => (deferrals.cap.resolve = r as never));
            if (kind === 'has_floating' && deferrals.hf)
                return new Promise((r) => (deferrals.hf.resolve = r as never));
            if (kind === 'layer_transform_capability') return Promise.resolve('none');
            if (kind === 'has_floating') return Promise.resolve(false);
            return Promise.resolve({});
        }),
        post: vi.fn(),
    };
    const fakeApp = {
        engine,
        activeLayerId: 5 as number | null,
        requestFrame: vi.fn(),
        transformModeMenu: null as unknown,
    };
    const gizmo = {
        active: false,
        commit: vi.fn(),
        attach: vi.fn(() => Promise.resolve(true)),
        frame: vi.fn(() => Promise.resolve()),
    };
    const TransformGizmoMock = vi.fn(function () {
        return gizmo;
    });
    return { engine, fakeApp, gizmo, TransformGizmoMock, deferrals };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../transform_gizmo', () => ({ TransformGizmo: TransformGizmoMock }));
vi.mock('../transform_bindings', () => ({
    floatingTransformBinding: () => ({ kind: 'floating' }),
    voidTransformBinding: (id: number) => ({ kind: 'void', id }),
}));

import { transformTool } from '../transform.svelte';
import {
    beginToolSession,
    killToolSession,
    toolEngine,
    ToolSessionCancelled,
} from '../tool_session';
import { withApi } from '../../engine/testApi';

// Layer a real transport + typed api over the fake engine's send/post spies so a
// `SessionEngine` can be begun over it.
withApi(engine);

const ctx = { canvasEl: {} } as never;

async function flush() {
    for (let i = 0; i < 6; i++) await Promise.resolve();
}

beforeEach(() => {
    engine.send.mockClear();
    engine.post.mockClear();
    gizmo.commit.mockClear();
    gizmo.attach.mockClear();
    gizmo.frame.mockClear();
    gizmo.active = false;
    fakeApp.activeLayerId = 5;
    delete deferrals.cap;
    delete deferrals.hf;
    killToolSession();
});

/**
 * Regression for the reported crash (`Cannot read properties of null (reading
 * 'frame')` in transform.onFrame): onFrame awaits an engine read, and a tool
 * switch runs onDeactivate (nulling the gizmo) + kills the session during that
 * await. The resumed read must reject via the dead session BEFORE the
 * `await gizmo.frame()` deref — so onFrame rejects with ToolSessionCancelled,
 * never a TypeError. (has_floating resolves `false`, so absent the session
 * reject the resumed onFrame would fall straight through to the null deref.)
 */
describe('transform onFrame resumes into a torn-down tool', () => {
    it('rejects with ToolSessionCancelled instead of dereferencing a null gizmo', async () => {
        deferrals.hf = { resolve: () => {} };
        beginToolSession(engine as never);
        // onActivate seeds the module gizmo; its own activate() sees cap 'none'
        // (eager) and no-ops.
        transformTool.onActivate?.(ctx);
        await flush();

        gizmo.active = false;
        const framePromise = transformTool.onFrame!() as unknown as Promise<void>;
        await flush(); // parked on the deferred has_floating

        // Tool switch: onDeactivate commits + nulls the gizmo; the session dies.
        transformTool.onDeactivate?.(ctx);
        killToolSession();

        deferrals.hf.resolve(false);

        await expect(framePromise).rejects.toBeInstanceOf(ToolSessionCancelled);
        expect(gizmo.frame).not.toHaveBeenCalled();
    });
});

/**
 * Regression for wrong-layer resumption: activate() captures the active layer
 * before its first await. If the active layer changes (and the session is
 * rebegun) while activate is parked, the captured op must NOT proceed against
 * the stale layer — the old session's next send rejects, so no `begin_transform`
 * / attach fires.
 */
describe('transform activate resumes after an active-layer change', () => {
    it('does not begin_transform/attach for the layer captured before the change', async () => {
        deferrals.cap = { resolve: () => {} };
        deferrals.hf = { resolve: () => {} };
        beginToolSession(engine as never);

        // activate() (via onActivate) captures layerId = 5, then parks on the
        // capability read.
        transformTool.onActivate?.(ctx);
        await flush();

        // Capability resolves 'destructive' while the session is still alive, so
        // activate advances to the has_floating read...
        deferrals.cap.resolve('destructive');
        await flush();

        // ...the active layer changes and the session is rebegun (as the
        // CanvasView layer-change effect does), killing the old session.
        fakeApp.activeLayerId = 6;
        beginToolSession(engine as never);
        expect(toolEngine()).not.toBeNull();

        // The parked has_floating read now resolves — but on the dead session, so
        // activate unwinds without issuing begin_transform or attaching.
        deferrals.hf.resolve(false);
        await flush();

        const beganTransform = engine.send.mock.calls.some((c) => c[0] === 'begin_transform');
        expect(beganTransform).toBe(false);
        expect(gizmo.attach).not.toHaveBeenCalled();
    });
});
