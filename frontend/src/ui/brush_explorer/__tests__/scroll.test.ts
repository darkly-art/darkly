import { describe, it, expect } from 'vitest';
import { approach, clampScroll, flingVelocity, glide, STOP_VELOCITY } from '../scroll';

describe('the owned scroll position', () => {
    it('stays inside the range', () => {
        expect(clampScroll(-40, 500)).toBe(0);
        expect(clampScroll(900, 500)).toBe(500);
        // A range shorter than nothing is still a range: a list with less
        // content than viewport pins at the top rather than going negative.
        expect(clampScroll(20, -100)).toBe(0);
    });
});

describe('a glide', () => {
    it('travels the same distance whatever the frame rate', () => {
        // The whole reason the decay is closed-form. A dropped frame must not
        // shorten a fling, or a flick would land somewhere different on a busy
        // machine than on an idle one.
        const smooth = (() => {
            let s = { y: 0, v: 2000 };
            for (let i = 0; i < 60; i++) s = glide(s.y, s.v, 1 / 60, 1e6);
            return s.y;
        })();
        const janky = (() => {
            let s = { y: 0, v: 2000 };
            for (let i = 0; i < 15; i++) s = glide(s.y, s.v, 1 / 15, 1e6);
            return s.y;
        })();
        expect(smooth).toBeCloseTo(janky, 0);
    });

    it('comes to rest rather than decaying forever', () => {
        let s = { y: 0, v: 1800 };
        for (let i = 0; i < 600 && s.v !== 0; i++) s = glide(s.y, s.v, 1 / 60, 1e6);
        // An exponential never reaches zero; without a floor the frame loop
        // would never sleep.
        expect(s.v).toBe(0);
    });

    it('stops dead at either end', () => {
        const top = glide(10, -4000, 1 / 60, 500);
        expect(top).toEqual({ y: 0, v: 0 });
        const bottom = glide(490, 4000, 1 / 60, 500);
        expect(bottom).toEqual({ y: 500, v: 0 });
    });

    it('drops a velocity already below the floor', () => {
        expect(glide(0, STOP_VELOCITY - 1, 1 / 60, 1e6).v).toBe(0);
    });
});

describe('a jump', () => {
    it('closes on its target and then reports arrival exactly', () => {
        let y = 0;
        for (let i = 0; i < 240 && y !== 900; i++) y = approach(y, 900, 1 / 60);
        // Exactly, not nearly: the frame loop uses equality to know the jump is
        // over and it can stop scheduling frames.
        expect(y).toBe(900);
    });

    it('moves the same distance whatever the frame rate', () => {
        let smooth = 0;
        for (let i = 0; i < 30; i++) smooth = approach(smooth, 1000, 1 / 60);
        let janky = 0;
        for (let i = 0; i < 15; i++) janky = approach(janky, 1000, 1 / 30);
        expect(smooth).toBeCloseTo(janky, 0);
    });
});

describe('a fling', () => {
    it('measures px per second over the tail of the travel', () => {
        const v = flingVelocity(
            [
                { t: 1000, y: 0 },
                { t: 1050, y: 50 },
            ],
            1050,
        );
        expect(v).toBeCloseTo(1000);
    });

    it('is nothing when the hand rested before letting go', () => {
        // Samples older than the window are ignored, so a pause before release
        // means a stop rather than whatever the gesture was doing earlier.
        const v = flingVelocity(
            [
                { t: 0, y: 0 },
                { t: 50, y: 400 },
            ],
            900,
        );
        expect(v).toBe(0);
    });

    it('is nothing from a single sample', () => {
        expect(flingVelocity([{ t: 10, y: 4 }], 10)).toBe(0);
    });
});
