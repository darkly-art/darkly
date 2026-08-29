import { describe, it, expect, vi, beforeEach } from 'vitest';

/**
 * `getDisplayMedia` needs transient user activation, which an `addVoid`
 * round-trip would expire. The void spawn therefore acquires the MediaStream
 * before it awaits anything else — an ordering constraint that is invisible in
 * the source and was untested before this modal lifted the body out of
 * `VoidPickerModal.svelte`.
 */

const calls: string[] = [];
const stopped = vi.fn();

let addVoidResult: number | null = 1;

const app = {
    engine: {
        api: {
            addVoid: vi.fn(async () => {
                calls.push('addVoid');
                return addVoidResult;
            }),
        },
    },
    activeLayerId: 7,
    acquireMediaStream: vi.fn(async () => {
        calls.push('acquire');
        return { getTracks: () => [{ stop: stopped }] } as unknown as MediaStream;
    }),
    selectLayer: vi.fn(),
    markStreamVoidStarted: vi.fn(() => calls.push('markStreamVoidStarted')),
    startStreamSource: vi.fn(async () => { calls.push('startStreamSource'); }),
    requestFrame: vi.fn(),
};

vi.mock('../../../state/app.svelte', () => ({ app }));

const { source } = await import('../addSources/voids');

function entry(over: Record<string, unknown> = {}) {
    return {
        type: 'screen',
        displayName: 'Screen',
        icon: null,
        description: null,
        category: null,
        hotkeyAction: null,
        params: [],
        supportsPreview: false,
        captureKind: 'display',
        addable: true,
        ...over,
    } as any;
}

beforeEach(() => {
    calls.length = 0;
    stopped.mockClear();
    addVoidResult = 1;
    vi.clearAllMocks();
});

describe('the void add source', () => {
    it('acquires the stream before awaiting addVoid', async () => {
        await source.spawn!(entry());
        expect(calls.indexOf('acquire')).toBeLessThan(calls.indexOf('addVoid'));
    });

    it('opts the new layer into the session allow-list and hands it the stream', async () => {
        await source.spawn!(entry());
        expect(calls).toEqual(['acquire', 'addVoid', 'markStreamVoidStarted', 'startStreamSource']);
        expect(app.selectLayer).toHaveBeenCalledWith(1);
    });

    it('releases the tracks when layer creation fails', async () => {
        addVoidResult = null;
        await source.spawn!(entry());
        expect(stopped).toHaveBeenCalledOnce();
        expect(app.markStreamVoidStarted).not.toHaveBeenCalled();
    });

    it('skips acquisition entirely for a void that needs no capture', async () => {
        await source.spawn!(entry({ type: 'noise', captureKind: null }));
        expect(app.acquireMediaStream).not.toHaveBeenCalled();
        expect(calls).toEqual(['addVoid']);
    });
});
