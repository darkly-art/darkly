import { describe, it, expect, vi, beforeEach } from 'vitest';

/**
 * `getDisplayMedia` needs transient user activation, which an `addVoid`
 * round-trip would expire. The void spawn therefore acquires the MediaStream
 * before it awaits anything else — an ordering constraint that is invisible in
 * the source, so it is pinned here.
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

const dispatch = vi.fn((id: string) => {
    calls.push(`dispatch:${id}`);
});

vi.mock('../../../state/app.svelte', () => ({ app }));
vi.mock('../../../actions/registry', () => ({ actions: { dispatch } }));

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
        source: { kind: 'capture', capture: 'display' },
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
        await source.spawn!(entry({ type: 'noise', source: { kind: 'procedural' } }));
        expect(app.acquireMediaStream).not.toHaveBeenCalled();
        expect(calls).toEqual(['addVoid']);
    });

    // An image void has no empty state to add — it needs a file first — so it
    // goes to the placement action rather than becoming a blank layer.
    it('hands an image-sourced void to the placement action', async () => {
        await source.spawn!(entry({ type: 'image', source: { kind: 'image' } }));
        expect(calls).toEqual(['dispatch:placeSmartObject']);
        expect(app.engine.api.addVoid).not.toHaveBeenCalled();
        expect(app.acquireMediaStream).not.toHaveBeenCalled();
    });
});
