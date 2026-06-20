import { describe, it, expect, vi, beforeEach } from 'vitest';

// Minimal stand-ins for the Svelte-runic state proxy and the gizmo, so the
// race in `dismissOverlay` can be driven without the Svelte/GPU runtime.
const { engine, fakeApp, gizmo, TransformGizmoMock } = vi.hoisted(() => {
    const engine = {
        send: vi.fn((kind: string) => {
            if (kind === 'has_floating') return Promise.resolve({ value: true });
            // A target layer that does NOT match activeLayerId, so dismissOverlay
            // falls through to the commit at the end (the crash site).
            if (kind === 'floating_target_layer') return Promise.resolve({ id: 999 });
            return Promise.resolve({});
        }),
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
    return { engine, fakeApp, gizmo, TransformGizmoMock };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../transform_gizmo', () => ({ TransformGizmo: TransformGizmoMock }));
vi.mock('../transform_bindings', () => ({
    floatingTransformBinding: () => ({}),
    voidTransformBinding: () => ({}),
}));

import { transformTool } from '../transform.svelte';

const ctx = { canvasEl: {} } as never;

async function flush() {
    for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
    gizmo.commit.mockClear();
    engine.send.mockClear();
    fakeApp.activeLayerId = 1;
});

/**
 * Regression: `dismissOverlay` is async and awaits two engine round-trips. A
 * tool switch / layer change can run `onDeactivate` (which commits and nulls
 * the gizmo) before those awaits resolve. The resumed `dismissOverlay` must not
 * dereference the now-null gizmo, nor commit a second time.
 */
describe('transform dismissOverlay race with onDeactivate', () => {
    it('does not throw or double-commit when deactivated mid-await', async () => {
        transformTool.onActivate?.(ctx);

        // Start the dismissal; its awaits are still pending at this point.
        const pending = transformTool.dismissOverlay!() as unknown as Promise<void>;

        // Tool gets torn down before the awaits resolve.
        transformTool.onDeactivate?.(ctx);
        expect(gizmo.commit).toHaveBeenCalledTimes(1); // onDeactivate's own commit

        // Resuming dismissOverlay must be a no-op, not a TypeError.
        await expect(pending).resolves.toBeUndefined();
        await flush();
        expect(gizmo.commit).toHaveBeenCalledTimes(1); // no double-commit
    });
});
