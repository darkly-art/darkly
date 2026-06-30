import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
    createTextFromPending,
    queueTextContent,
    flushTextContent,
    dispatchStyle,
    shouldReseed,
    type EditorHost,
} from '../text_editor';

// Minimal engine + host fakes (no DOM): record `send`/`post` and stand in for
// the app's selection / refresh / frame plumbing.
function fakeHost() {
    const engine = {
        send: vi.fn((kind: string) => {
            if (kind === 'add_text') return Promise.resolve({ id: 7, object: 3 });
            if (kind === 'text_objects') return Promise.resolve({ objects: [{ object: 3 }] });
            return Promise.resolve(null);
        }),
        post: vi.fn(),
    };
    const host: EditorHost = {
        engine: engine as never,
        selectLayer: vi.fn(),
        refreshLayerTree: vi.fn(() => Promise.resolve()),
        requestFrame: vi.fn(),
    };
    return { engine, host };
}

const STYLE = { font_family: 'Noto Sans', size: 48, weight: 400, italic: false, align: 'start' };
const PLACEMENT = { x: 100, y: 60, anchorLayerId: 4 };

beforeEach(() => {
    flushTextContent(); // drain any queue left by a prior test
});

describe('text editor logic', () => {
    it('deferred-create posts add_text then selects the returned layer', async () => {
        const { engine, host } = fakeHost();
        const r = await createTextFromPending(
            host,
            PLACEMENT,
            'Hi',
            STYLE,
            [10, 20, 30, 255],
            () => 'Hi',
        );
        expect(engine.send).toHaveBeenCalledWith(
            'add_text',
            expect.objectContaining({
                content: 'Hi',
                x: 100,
                y: 60,
                size: 48,
                font_family: 'Noto Sans',
                align: 'start',
                italic: false,
                weight: 400,
                color: [10, 20, 30, 255],
                anchor: 4,
            }),
        );
        expect(host.selectLayer).toHaveBeenCalledWith(7);
        expect(r).toEqual({ layerId: 7, objectId: 3, latest: 'Hi' });
    });

    it('syncs characters typed during the create await via set_text_content', async () => {
        const { engine, host } = fakeHost();
        // `latest()` reports more than the value add_text was seeded with.
        await createTextFromPending(host, PLACEMENT, 'H', STYLE, [0, 0, 0, 255], () => 'Hello');
        expect(engine.post).toHaveBeenCalledWith('set_text_content', {
            id: 7,
            object: 3,
            content: 'Hello',
        });
    });

    it('typing on a bound object posts set_text_content { id, object, content }', () => {
        const { engine, host } = fakeHost();
        queueTextContent(host, 42, 3, 'updated');
        flushTextContent();
        expect(engine.post).toHaveBeenCalledWith('set_text_content', {
            id: 42,
            object: 3,
            content: 'updated',
        });
    });

    it('coalesces multiple content writes to the latest value per object', () => {
        const { engine, host } = fakeHost();
        queueTextContent(host, 42, 3, 'a');
        queueTextContent(host, 42, 3, 'ab');
        queueTextContent(host, 42, 3, 'abc');
        flushTextContent();
        const contentCalls = engine.post.mock.calls.filter((c) => c[0] === 'set_text_content');
        expect(contentCalls).toHaveLength(1);
        expect(contentCalls[0][1]).toEqual({ id: 42, object: 3, content: 'abc' });
    });

    it('a style change posts set_text_style and updates the placement defaults', () => {
        const { engine, host } = fakeHost();
        const defaults: Record<string, unknown> = {};
        dispatchStyle(host, 42, 3, { font_family: 'Serif', align: 'center' }, defaults);
        expect(engine.post).toHaveBeenCalledWith('set_text_style', {
            id: 42,
            object: 3,
            font_family: 'Serif',
            align: 'center',
        });
        expect(defaults).toEqual({ fontFamily: 'Serif', align: 'center' });
    });

    it('re-seeds only on an external change, not a self-echo', () => {
        // Undo/redo: engine content differs from what we last sent → re-seed.
        expect(shouldReseed('reverted', 'typed')).toBe(true);
        // Our own echo bouncing back on a refresh → leave the field untouched.
        expect(shouldReseed('typed', 'typed')).toBe(false);
    });
});
