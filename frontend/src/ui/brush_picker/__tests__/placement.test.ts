import { describe, expect, it } from 'vitest';
import { brushPickerPlacement } from '../placement';

describe('brush picker placement', () => {
    it('opens below a trigger in the fullscreen toolbar', () => {
        expect(brushPickerPlacement(
            { left: 24, top: 8, right: 180, bottom: 40 },
            { width: 1200, height: 800 },
            480,
        )).toEqual({ left: 24, top: 46, bottom: null });
    });

    it('opens above a trigger in the docked bottom toolbar', () => {
        expect(brushPickerPlacement(
            { left: 24, top: 752, right: 180, bottom: 784 },
            { width: 1200, height: 800 },
            480,
        )).toEqual({ left: 24, top: null, bottom: 54 });
    });

    it('clamps its horizontal position into the viewport', () => {
        expect(brushPickerPlacement(
            { left: 900, top: 752, right: 1056, bottom: 784 },
            { width: 1000, height: 800 },
            480,
        ).left).toBe(512);
    });
});
