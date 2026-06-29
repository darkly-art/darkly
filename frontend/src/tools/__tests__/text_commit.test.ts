import { describe, it, expect } from 'vitest';
import { buildCommit, type EditState } from '../text_commit';

const STYLE = { size: 48, fontFamily: 'Noto Sans', align: 'start', italic: false, weight: 400 };
const COLOR = { r: 10, g: 20, b: 30, a: 255 };

const placing: EditState = { layerId: null, objectId: null, cx: 100, cy: 60, anchorLayerId: 7 };
const editing: EditState = { layerId: 42, objectId: 3, cx: 0, cy: 0, anchorLayerId: null };

describe('text tool commit FSM', () => {
    it('cancels on empty content (no empty layer is created)', () => {
        expect(buildCommit(placing, '', STYLE, COLOR).kind).toBe('cancel');
        expect(buildCommit(placing, '   \n  ', STYLE, COLOR).kind).toBe('cancel');
    });

    it('placing a new block with content emits add_text with caret + style + color', () => {
        const req = buildCommit(placing, 'Hello', STYLE, COLOR);
        expect(req.kind).toBe('add_text');
        if (req.kind !== 'add_text') throw new Error('unreachable');
        expect(req.payload).toMatchObject({
            content: 'Hello',
            x: 100,
            y: 60,
            size: 48,
            font_family: 'Noto Sans',
            align: 'start',
            italic: false,
            weight: 400,
            color: [10, 20, 30, 255],
            anchor: 7,
        });
    });

    it('placing with no active layer anchors at -1', () => {
        const req = buildCommit({ ...placing, anchorLayerId: null }, 'Hi', STYLE, COLOR);
        if (req.kind !== 'add_text') throw new Error('expected add_text');
        expect(req.payload.anchor).toBe(-1);
    });

    it('editing an existing object emits set_text_content for that id + object', () => {
        const req = buildCommit(editing, 'Updated', STYLE, COLOR);
        expect(req).toEqual({
            kind: 'set_text_content',
            payload: { id: 42, object: 3, content: 'Updated' },
        });
    });

    it('placing a new block carries objectId: null through to add_text', () => {
        const req = buildCommit(placing, 'New', STYLE, COLOR);
        expect(req.kind).toBe('add_text');
        expect(placing.objectId).toBeNull();
    });
});
