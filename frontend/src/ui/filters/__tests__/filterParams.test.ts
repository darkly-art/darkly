import { describe, it, expect } from 'vitest';
import { partitionFilterParams, type FilterParam } from '../filterParams';

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
    it("groups the curves layer's five channels under the selector", () => {
        const params = ['red', 'green', 'blue', 'value', 'alpha'].map(curve);
        const { curves, scalars } = partitionFilterParams(params);
        expect(curves.map((c) => c.name)).toEqual(['red', 'green', 'blue', 'value', 'alpha']);
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
