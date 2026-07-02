import { describe, it, expect, vi, beforeEach } from 'vitest';

// Stand-in for the Svelte-runic `app` proxy: records `post` calls and answers
// the one `send` the binding makes on first read.
const { engine, fakeApp } = vi.hoisted(() => {
    const engine = {
        send: vi.fn((_kind: string, _payload?: object) =>
            Promise.resolve({ ox: 0, oy: 0, w: 10, h: 4, mode: 0, matrix: [1, 0, 5, 0, 1, 7] }),
        ),
        post: vi.fn(),
    };
    const fakeApp = { engine };
    return { engine, fakeApp };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));

import { vectorObjectTransformBinding } from '../transform_bindings';
import { beginToolSession } from '../tool_session';
import type { Mat3 } from '../transform_projective';

// The bindings reach the engine through the live tool session; establish one
// over the fake engine so `toolEngine()` resolves (its send/post delegate to
// the fake's spies).
beforeEach(() => {
    engine.send.mockClear();
    engine.post.mockClear();
    beginToolSession(engine as never);
});

describe('vectorObjectTransformBinding', () => {
    it('reads via vector_object_info and posts updates with id + object + payload', async () => {
        const binding = vectorObjectTransformBinding(42, 3);
        const geo = await binding.read();
        const [readKind, readPayload] = engine.send.mock.calls[0];
        expect(readKind).toBe('vector_object_info');
        expect(readPayload).toEqual({ id: 42, object: 3 });
        expect(geo).toEqual({ origin: [0, 0], w: 10, h: 4, mode: 0, matrix: [1, 0, 5, 0, 1, 7, 0, 0, 1] });

        const next: Mat3 = [2, 0, 9, 0, 2, 11, 0, 0, 1];
        binding.update(next, 0);
        const [updateKind, updatePayload] = engine.post.mock.calls[0];
        expect(updateKind).toBe('update_vector_object_transform');
        expect(updatePayload).toEqual({
            id: 42,
            object: 3,
            mode_tag: 0,
            payload: [2, 0, 9, 0, 2, 11],
        });
    });

    it('cancel re-posts the affine captured on first read', async () => {
        const binding = vectorObjectTransformBinding(42, 3);
        await binding.read(); // captures [1,0,5,0,1,7,0,0,1] as the original
        binding.update([2, 0, 9, 0, 2, 11, 0, 0, 1], 0);
        engine.post.mockClear();

        binding.cancel();
        const [kind, payload] = engine.post.mock.calls[0];
        expect(kind).toBe('update_vector_object_transform');
        expect(payload).toEqual({
            id: 42,
            object: 3,
            mode_tag: 0,
            payload: [1, 0, 5, 0, 1, 7],
        });
    });
});
