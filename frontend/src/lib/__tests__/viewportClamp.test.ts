// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { clampToViewport } from '../viewportClamp';

beforeEach(() => {
    window.innerWidth = 1000;
    window.innerHeight = 800;
});

describe('clampToViewport', () => {
    it('leaves a position that already fits alone', () => {
        expect(clampToViewport(100, 100, { width: 160, height: 200 })).toEqual({ x: 100, y: 100 });
    });

    it('pulls a surface back from the right and bottom edges', () => {
        expect(clampToViewport(950, 750, { width: 160, height: 200 })).toEqual({
            x: 1000 - 8 - 160,
            y: 800 - 8 - 200,
        });
    });

    it('keeps the margin at the top and left', () => {
        expect(clampToViewport(-50, -50, { width: 160, height: 200 })).toEqual({ x: 8, y: 8 });
    });

    it('pins a surface larger than the viewport to the top-left', () => {
        expect(clampToViewport(500, 500, { width: 2000, height: 2000 })).toEqual({ x: 8, y: 8 });
    });
});
