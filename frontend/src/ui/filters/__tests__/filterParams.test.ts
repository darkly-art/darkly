import { describe, it, expect } from 'vitest';
import {
    partitionFilterParams,
    channelLabel,
    colorizeActive,
    seedScratchParams,
    filterParamMap,
    cloneParamValue,
    paramIsResettable,
    listItemSchema,
    newListEntry,
    type ParamInfo,
    type ListValue,
} from '../filterParams';


/** Fill the fields every `ParamInfo` carries but these cases don't exercise, so
 *  each fixture below states only what it is actually testing. Keeping the
 *  fixtures typed as the generated `ParamInfo` is the point: this file is the
 *  guard that the generated type still satisfies every panel helper. */
function param(p: Partial<ParamInfo> & Pick<ParamInfo, 'kind' | 'name' | 'default'>): ParamInfo {
    return {
        label: null,
        description: null,
        widget: 'auto',
        unit: 'Raw',
        min: null,
        max: null,
        value: null,
        options: null,
        display: { min: null, max: null, default: null, unit: '' },
        ...p,
    };
}

const curve = (name: string): ParamInfo => param({
    kind: 'curve',
    name,
    default: [
        [0, 0],
        [1, 1],
    ],
});
const levels = (name: string): ParamInfo => param({
    kind: 'levels',
    name,
    default: [0, 1, 1, 0, 1],
});
const scalar = (name: string): ParamInfo => param({ kind: 'float', name, default: 0 });

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
const enumParam = (): ParamInfo => param({
    kind: 'enum',
    name: 'model',
    default: 0,
    options: ['HSV', 'HSL', 'HSY'],
});
const bool = (name: string, def = false): ParamInfo => param({ kind: 'bool', name, default: def });
const hsvSchema = (): ParamInfo[] => [
    enumParam(),
    param({ kind: 'float', name: 'hue', min: -180, max: 180, default: 0 }),
    param({ kind: 'float', name: 'saturation', min: -100, max: 100, default: 0 }),
    param({ kind: 'float', name: 'value', min: -100, max: 100, default: 0 }),
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
        const schema: ParamInfo[] = [
            param({ kind: 'curve', name: 'rgb', default: [[0, 0], [1, 1]] }),
            param({ kind: 'float', name: 'hue', default: 30 }),
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

// The chromatic-aberration schema shape: one `list` param whose `options`
// carries the per-entry item schema (vec2/float/color/float).
const itemSchema = (): ParamInfo[] => [
    param({ kind: 'vec2', name: 'offset', max: 64, default: [0, 0] }),
    param({ kind: 'float', name: 'scale', min: 0.9, max: 1.1, default: 1 }),
    param({ kind: 'color', name: 'color', default: [1, 1, 1] }),
    param({ kind: 'float', name: 'blur', min: 0, max: 6, default: 0 }),
];
const listParam = (): ParamInfo => param({
    kind: 'list',
    name: 'aberrations',
    max: 16,
    default: [
        { offset: [0, 0], scale: 1.004, color: [1, 0, 0], blur: 0.6 },
        { offset: [0, 0], scale: 1, color: [0, 1, 0], blur: 0.6 },
    ],
    options: itemSchema(),
});

describe('cloneParamValue', () => {
    it('deep-clones list entries (modal scratch-copy regression guard)', () => {
        const original = (listParam().default as ListValue);
        const copy = cloneParamValue(original);
        // Mutating a scalar field of a cloned entry must not touch the original.
        copy[0].scale = 2;
        expect(original[0].scale).toBe(1.004);
        // Mutating a nested array field (the offset vec2) must not alias either.
        (copy[1].offset as number[])[0] = 9;
        expect((original[1].offset as number[])[0]).toBe(0);
    });

    it('still deep-clones curve pairs', () => {
        const curvePts: [number, number][] = [[0, 0], [1, 1]];
        const copy = cloneParamValue(curvePts);
        copy[0][0] = 0.5;
        expect(curvePts[0][0]).toBe(0);
    });
});

describe('list param helpers', () => {
    it('listItemSchema exposes the per-entry item schema', () => {
        const schema = listItemSchema(listParam());
        expect(schema.map((p) => p.name)).toEqual(['offset', 'scale', 'color', 'blur']);
        expect(schema.map((p) => p.kind)).toEqual(['vec2', 'float', 'color', 'float']);
    });

    it('listItemSchema is empty for a non-list param (enum options are labels)', () => {
        expect(listItemSchema(enumParam())).toEqual([]);
    });

    it('newListEntry seeds each field from its item default', () => {
        const entry = newListEntry(listItemSchema(listParam()));
        expect(entry).toEqual({ offset: [0, 0], scale: 1, color: [1, 1, 1], blur: 0 });
        // The seeded arrays are fresh clones, not the schema's shared defaults.
        const schema = itemSchema();
        (entry.offset as number[])[0] = 5;
        expect((schema[0].default as number[])[0]).toBe(0);
    });

    it('filterParamMap passes a list value through (value ?? default)', () => {
        const p = listParam();
        expect(filterParamMap([p])).toEqual({ aberrations: p.default });
        p.value = [{ offset: [2, 0], scale: 1.01, color: [1, 0, 0], blur: 0 }];
        expect(filterParamMap([p])).toEqual({ aberrations: p.value });
    });

    it('partitions a list param into scalars (routed to the list editor)', () => {
        const { channels, scalars } = partitionFilterParams([listParam()]);
        expect(channels).toHaveLength(0);
        expect(scalars.map((s) => s.kind)).toEqual(['list']);
    });
});

describe('paramIsResettable', () => {
    it('is true for the offset pad and the interior-default scale slider', () => {
        const offset: ParamInfo = param({ kind: 'vec2', name: 'offset', max: 64, default: [0, 0] });
        const scale: ParamInfo = param({ kind: 'float', name: 'scale', min: 0.9, max: 1.1, default: 1 });
        expect(paramIsResettable(offset)).toBe(true);
        expect(paramIsResettable(scale)).toBe(true);
    });

    it('is false for an endpoint-default slider (blur) and a deliberate pick (color)', () => {
        const blur: ParamInfo = param({ kind: 'float', name: 'blur', min: 0, max: 6, default: 0 });
        const color: ParamInfo = param({ kind: 'color', name: 'color', default: [1, 1, 1] });
        expect(paramIsResettable(blur)).toBe(false);
        expect(paramIsResettable(color)).toBe(false);
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
