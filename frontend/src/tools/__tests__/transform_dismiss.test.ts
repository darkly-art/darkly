import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Minimal stand-ins for the Svelte-runic state proxy and the gizmo, so the
// race in `dismissOverlay` can be driven without the Svelte/GPU runtime.
const { engine, fakeApp, gizmo, TransformGizmoMock, resolveHasFloating } = vi.hoisted(() => {
    let resolve: (v: boolean) => void = () => {};
    const engine = {
        // has_floating is deferred so dismissOverlay parks on its await, giving
        // the test a window to tear the tool down before it resolves. The typed
        // api returns bare values, so the mock resolves a bare boolean / id.
        send: vi.fn((kind: string) => {
            if (kind === 'has_floating') return new Promise<boolean>((r) => (resolve = r));
            // A target layer that does NOT match activeLayerId, so a resumed
            // dismissOverlay would fall through to the commit at the end.
            if (kind === 'floating_target_layer') return Promise.resolve(999);
            return Promise.resolve({});
        }),
        post: vi.fn(),
    };
    const fakeApp = {
        engine,
        activeLayerId: 1 as number | null,
        requestFrame: vi.fn(),
    };
    const gizmo = { active: false, commit: vi.fn(), attach: vi.fn() };
    const TransformGizmoMock = vi.fn(function () {
        return gizmo;
    });
    return { engine, fakeApp, gizmo, TransformGizmoMock, resolveHasFloating: () => resolve(true) };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../transform_gizmo', () => ({ TransformGizmo: TransformGizmoMock }));
vi.mock('../transform_bindings', () => ({
    floatingTransformBinding: () => ({}),
    voidTransformBinding: () => ({}),
}));

import { transformTool } from '../transform.svelte';
import { beginToolSession, killToolSession, runHook, ToolSessionCancelled } from '../tool_session';

// Attach a real transport + typed api over the fake engine's send/post spies so
// a `SessionEngine` can be begun over it (the bindings reach the engine only
// through the live session).
withApi(engine);

const ctx = { canvasEl: {} } as never;

async function flush() {
    for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
    gizmo.commit.mockClear();
    engine.send.mockClear();
    fakeApp.activeLayerId = 1;
    beginToolSession(engine as never);
});

/**
 * Regression: `dismissOverlay` is async and awaits engine round-trips. A tool
 * switch / layer change tears the tool down (onDeactivate commits + nulls the
 * gizmo) AND kills the tool session in the same synchronous moment. The parked
 * dismissOverlay must then reject via the dead session — unwinding before it can
 * dereference the now-null gizmo or commit a second time — and that rejection is
 * a `ToolSessionCancelled` that the dispatcher's `runHook` swallows.
 */
describe('transform dismissOverlay race with session teardown', () => {
    it('rejects with ToolSessionCancelled instead of double-committing', async () => {
        transformTool.onActivate?.(ctx);

        // Start the dismissal; it parks on the deferred has_floating.
        const pending = transformTool.dismissOverlay!() as unknown as Promise<void>;

        // Tool torn down before the await resolves: onDeactivate commits once and
        // nulls the gizmo; the session dies alongside it.
        transformTool.onDeactivate?.(ctx);
        killToolSession();
        expect(gizmo.commit).toHaveBeenCalledTimes(1); // onDeactivate's own commit

        // The deferred read now resolves — but on a dead session, so the resumed
        // dismissOverlay rejects rather than reaching its final commit.
        resolveHasFloating();
        await expect(pending).rejects.toBeInstanceOf(ToolSessionCancelled);
        await flush();
        expect(gizmo.commit).toHaveBeenCalledTimes(1); // no double-commit

        // Wrapped by the dispatcher's runHook, that same rejection settles cleanly.
        await expect(runHook(Promise.reject(new ToolSessionCancelled()))).resolves.toBeUndefined();
    });
});
