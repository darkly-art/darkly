import { describe, it, expect } from 'vitest';
import {
    partitionFilterParams,
    channelLabel,
    colorizeActive,
    seedScratchParams,
    filterParamMap,
    type FilterParam,
} from '../filterParams';

const curve = (name: string): FilterParam => ({
    kind: 'curve',
    name,
    default: [
        [0, 0],
        [1, 1],
    ],
});
const levels = (name: string): FilterParam => ({
    kind: 'levels',
    name,
    default: [0, 1, 1, 0, 1],
});
const scalar = (name: string): FilterParam => ({ kind: 'float', name, default: 0 });

describe('partitionFilterParams', () => {
    it("groups the curves layer's eight Krita channels under the selector", () => {
        const names = ['rgb', 'red', 'green', 'blue', 'alpha', 'hue', 'saturation', 'lightness'];
        const params = names.map(curve);
        const { channels, scalars } = partitionFilterParams(params);
        expect(channels.map((c) => c.name)).toEqual(names);
        expect(scalars).toHaveLength(0);
    });

    it("groups the levels layer's channels under the same selector", () => {
        const names = ['rgb', 'red', 'green', 'blue', 'alpha', 'hue', 'saturation', 'lightness'];
        const params = names.map(levels);
        const { channels, scalars } = partitionFilterParams(params);
        expect(channels.map((c) => c.name)).toEqual(names);
        expect(scalars).toHaveLength(0);
    });

    it('separates channel params from scalars while preserving order', () => {
        const params = [scalar('amount'), curve('red'), scalar('mix'), levels('blue')];
        const { channels, scalars } = partitionFilterParams(params);
        expect(channels.map((c) => c.name)).toEqual(['red', 'blue']);
        expect(scalars.map((s) => s.name)).toEqual(['amount', 'mix']);
    });

    it('handles a parameter-free filter (invert)', () => {
        const { channels, scalars } = partitionFilterParams([]);
        expect(channels).toHaveLength(0);
        expect(scalars).toHaveLength(0);
    });
});

// The HSV filter's schema: an enum model, three scalars, and a colorize bool.
const enumParam = (): FilterParam => ({
    kind: 'enum',
    name: 'model',
    default: 0,
    options: ['HSV', 'HSL', 'HSY'],
});
const bool = (name: string, def = false): FilterParam => ({ kind: 'bool', name, default: def });
const hsvSchema = (): FilterParam[] => [
    enumParam(),
    { kind: 'float', name: 'hue', min: -180, max: 180, default: 0 },
    { kind: 'float', name: 'saturation', min: -100, max: 100, default: 0 },
    { kind: 'float', name: 'value', min: -100, max: 100, default: 0 },
    bool('colorize'),
];

describe('enum params', () => {
    it('partitions the model enum to scalars, carrying its options', () => {
        const { channels, scalars } = partitionFilterParams(hsvSchema());
        expect(channels).toHaveLength(0);
        const model = scalars.find((s) => s.name === 'model');
        expect(model?.kind).toBe('enum');
        expect(model?.options).toEqual(['HSV', 'HSL', 'HSY']);
    });
});

describe('colorizeActive', () => {
    it('is false at defaults and true once colorize is set', () => {
        const schema = hsvSchema();
        expect(colorizeActive(schema)).toBe(false);
        const on = seedScratchParams(schema);
        on.find((p) => p.name === 'colorize')!.value = true;
        expect(colorizeActive(on)).toBe(true);
    });
});

describe('seedScratchParams', () => {
    it('seeds each value from a deep-cloned default (no aliasing)', () => {
        const schema: FilterParam[] = [
            { kind: 'curve', name: 'rgb', default: [[0, 0], [1, 1]] },
            { kind: 'float', name: 'hue', default: 30 },
        ];
        const scratch = seedScratchParams(schema);
        expect(scratch.map((p) => p.value)).toEqual([[[0, 0], [1, 1]], 30]);
        // Mutating the scratch curve must not touch the schema default.
        (scratch[0].value as number[][])[0][0] = 0.5;
        expect((schema[0].default as number[][])[0][0]).toBe(0);
    });

    it('round-trips through filterParamMap to a name→value record', () => {
        const scratch = seedScratchParams(hsvSchema());
        scratch.find((p) => p.name === 'hue')!.value = 120;
        expect(filterParamMap(scratch)).toEqual({
            model: 0,
            hue: 120,
            saturation: 0,
            value: 0,
            colorize: false,
        });
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
