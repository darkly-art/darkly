import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Engine } from '../../engine/protocol';
import { DarklyInstance } from '../app.svelte';

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((done) => (resolve = done));
    return { promise, resolve };
}

describe('isolation acknowledgement', () => {
    let instance: DarklyInstance;

    beforeEach(() => {
        vi.stubGlobal('requestAnimationFrame', vi.fn());
        instance = new DarklyInstance();
    });

    it('uses the isolation id confirmed by the engine', async () => {
        instance.isolatedNodeId = 4;
        instance.engine = {
            api: { setIsolatedNode: vi.fn().mockResolvedValue(4) },
        } as unknown as Engine;

        await instance.setIsolatedNode(9);

        expect(instance.isolatedNodeId).toBe(4);
    });

    it('ignores a stale response from an older request', async () => {
        const first = deferred<number | null>();
        const second = deferred<number | null>();
        instance.engine = {
            api: {
                setIsolatedNode: vi
                    .fn()
                    .mockReturnValueOnce(first.promise)
                    .mockReturnValueOnce(second.promise),
            },
        } as unknown as Engine;

        const older = instance.setIsolatedNode(4);
        const newer = instance.setIsolatedNode(9);
        second.resolve(9);
        await newer;
        first.resolve(4);
        await older;

        expect(instance.isolatedNodeId).toBe(9);
    });
});
