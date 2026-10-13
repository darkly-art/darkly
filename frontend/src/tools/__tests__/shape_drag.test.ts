import { describe, it, expect } from 'vitest';
import { buildBeginShape, dragBox } from '../shape_editor';

const plain = { square: false, fromCenter: false };

describe('dragBox', () => {
    it('spans the press and current points', () => {
        expect(dragBox(10, 20, 110, 70, plain)).toEqual({ x: 10, y: 20, w: 100, h: 50 });
    });

    it('normalises a drag up and to the left', () => {
        expect(dragBox(110, 70, 10, 20, plain)).toEqual({ x: 10, y: 20, w: 100, h: 50 });
    });

    it('Shift squares on the longer side, keeping the drag direction', () => {
        const mods = { square: true, fromCenter: false };
        expect(dragBox(10, 20, 110, 70, mods)).toEqual({ x: 10, y: 20, w: 100, h: 100 });
        expect(dragBox(100, 100, 70, 40, mods)).toEqual({ x: 40, y: 40, w: 60, h: 60 });
    });

    it('Alt grows the box from the press point as centre', () => {
        const mods = { square: false, fromCenter: true };
        expect(dragBox(50, 50, 80, 60, mods)).toEqual({ x: 20, y: 40, w: 60, h: 20 });
        expect(dragBox(50, 50, 20, 40, mods)).toEqual({ x: 20, y: 40, w: 60, h: 20 });
    });

    it('Shift and Alt together give a square centred on the press point', () => {
        const mods = { square: true, fromCenter: true };
        expect(dragBox(50, 50, 80, 60, mods)).toEqual({ x: 20, y: 20, w: 60, h: 60 });
    });
});

describe('buildBeginShape', () => {
    it('carries the box, family, params and style', () => {
        const req = buildBeginShape(
            { x: 1, y: 2, w: 30, h: 40 },
            'polygon',
            { params: { points: 6 }, fill: [1, 2, 3, 255], border: { width: 0, color: [0, 0, 0, 255] } },
            7,
        );
        expect(req).toEqual({
            family: 'polygon',
            params: { points: 6 },
            x: 1,
            y: 2,
            width: 30,
            height: 40,
            fill: [1, 2, 3, 255],
            stroke: { width: 0, color: [0, 0, 0, 255] },
            anchor: 7,
        });
    });
});
