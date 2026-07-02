import { describe, it, expect } from 'vitest';
import { partitionFilterParams, channelLabel, type FilterParam } from '../filterParams';

const curve = (name: string): FilterParam => ({
    kind: 'curve',
    name,
    default: [
        [0, 0],
        [1, 1],
    ],
});
const scalar = (name: string): FilterParam => ({ kind: 'float', name, default: 0 });

describe('partitionFilterParams', () => {
    it("groups the curves layer's eight Krita channels under the selector", () => {
        const names = ['rgb', 'red', 'green', 'blue', 'alpha', 'hue', 'saturation', 'lightness'];
        const params = names.map(curve);
        const { curves, scalars } = partitionFilterParams(params);
        expect(curves.map((c) => c.name)).toEqual(names);
        expect(scalars).toHaveLength(0);
    });

    it('separates curves from scalars while preserving order', () => {
        const params = [scalar('amount'), curve('red'), scalar('mix'), curve('blue')];
        const { curves, scalars } = partitionFilterParams(params);
        expect(curves.map((c) => c.name)).toEqual(['red', 'blue']);
        expect(scalars.map((s) => s.name)).toEqual(['amount', 'mix']);
    });

    it('handles a parameter-free filter (invert)', () => {
        const { curves, scalars } = partitionFilterParams([]);
        expect(curves).toHaveLength(0);
        expect(scalars).toHaveLength(0);
    });
});

describe('channelLabel', () => {
    it('renders the RGB composite acronym uppercase, not title-cased', () => {
        expect(channelLabel('rgb')).toBe('RGB');
    });

    it('title-cases the ordinary channels', () => {
        expect(channelLabel('red')).toBe('Red');
        expect(channelLabel('saturation')).toBe('Saturation');
        expect(channelLabel('lightness')).toBe('Lightness');
    });
});
