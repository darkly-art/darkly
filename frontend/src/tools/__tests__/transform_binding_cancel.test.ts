import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// The void binding captures the pre-edit transform — INCLUDING its mode — on
// first read, so cancelling reverts faithfully. Engine transport is mocked; the
// real typed `api` forwards to the `send`/`post` spies, so `cancel` crosses the
// wire as `updateVoidTransform({ id, transform: { mode, data } })`.
const { fakeApp } = vi.hoisted(() => {
    const engine = { post: vi.fn(), send: vi.fn() };
    return { fakeApp: { engine } };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));

import { voidTransformBinding } from '../transform_bindings';
import { SessionEngine } from '../tool_session';
import type { Mat3 } from '../transform_projective';

// Attach a real transport + typed api to the fake engine, then open a session
// over it so the bindings' session accessor resolves — its api forwards to the
// same `send`/`post` spies, so assertions still inspect them by kind.
withApi(fakeApp.engine);
let session: SessionEngine | null = null;
const sess = () => session;
beforeEach(() => {
    session = new SessionEngine(fakeApp.engine as never);
});

describe('voidTransformBinding cancel preserves the original mode', () => {
    it('reverts a void that was already perspective to mode 1, not affine', async () => {
        const homography: Mat3 = [1, 0.1, 4, 0.2, 1, -3, 0.001, 0.002, 1];
        fakeApp.engine.send.mockResolvedValue({
            ox: 0,
            oy: 0,
            w: 100,
            h: 80,
            mode: 1,
            matrix: [...homography],
        });

        const binding = voidTransformBinding(sess, 42);
        await binding.read(); // captures { matrix, mode: 1 }
        binding.update([1, 0, 0, 0, 1, 0, 0, 0, 1] as Mat3, 0); // some live edit

        fakeApp.engine.post.mockClear();
        binding.cancel();

        expect(fakeApp.engine.post).toHaveBeenCalledTimes(1);
        const [kind, payload] = fakeApp.engine.post.mock.calls[0];
        expect(kind).toBe('update_void_transform');
        expect(payload.transform.mode).toBe('Perspective');
        expect(payload.transform.data.length).toBe(9);
        expect(payload.transform.data).toEqual([...homography]);
    });

    it('reverts an affine void to mode 0 with the 6-float payload', async () => {
        const affineMat3: Mat3 = [2, 0, 5, 0, 2, 9, 0, 0, 1];
        fakeApp.engine.send.mockResolvedValue({
            ox: 0,
            oy: 0,
            w: 100,
            h: 80,
            mode: 0,
            matrix: [2, 0, 5, 0, 2, 9],
        });

        const binding = voidTransformBinding(sess, 7);
        await binding.read();
        fakeApp.engine.post.mockClear();
        binding.cancel();

        const [, payload] = fakeApp.engine.post.mock.calls[0];
        expect(payload.transform.mode).toBe('Basic');
        expect(payload.transform.data).toEqual([2, 0, 5, 0, 2, 9]);
        // (the lifted Mat3 round-trips back to its affine wire form)
        expect(affineMat3[8]).toBe(1);
    });
});
