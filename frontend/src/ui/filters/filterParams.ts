/**
 * Pure helpers for the filter-layer properties panel.
 *
 * A filter's params (the `ParamInfo[]` emitted by the Rust `filter_types()` /
 * layer-tree query) mix scalar controls (sliders, checkboxes) with per-channel
 * tone params. Both the `curve` (Curves) and `levels` (Levels) kinds expose the
 * same eight virtual channels and share the channel selector, differing only in
 * their editor widget. The panel renders every channel param through one shared
 * selector + a `<CurveEditor>`/`<LevelsEditor>`; the scalars render as their own
 * rows. This module owns the split so the component stays declarative and the
 * grouping is unit-testable.
 */

export type CurvePoints = [number, number][];
/** `[inBlack, inWhite, gamma, outBlack, outWhite]` — a Levels transfer. */
export type LevelsValues = [number, number, number, number, number];
/** Normalized sRGB `[r, g, b]` in `[0,1]` — a `ParamValue::Color`. */
export type ColorValue = [number, number, number];
/** A 2D vector `[x, y]` — a `ParamValue::Vec2` (offset pad). */
export type Vec2Value = [number, number];
/** A dynamic list of named-value entries — a `ParamValue::List`. */
export type ListValue = Record<string, FilterParamValue>[];

/** A concrete param value on the wire (matches `ParamValue` in `protocol_gen`). */
export type FilterParamValue =
    | number
    | boolean
    | CurvePoints
    | LevelsValues
    | ColorValue
    | Vec2Value
    | ListValue;

/** One entry of a filter's `params` array — a `ParamInfo` view. */
export interface FilterParam {
    kind: string;
    name: string;
    min?: number;
    max?: number;
    default: FilterParamValue;
    value?: FilterParamValue;
    /** Enum: the option labels (stored value is the index into this).
     *  List: the item schema, one `FilterParam` per entry field. */
    options?: string[] | FilterParam[];
}

/** Kinds that are per-channel tone params, sharing the channel selector. */
export function isChannelParam(kind: string): boolean {
    return kind === 'curve' || kind === 'levels';
}

/** Deep-clone a param value (curve pairs / levels arrays / list entries can't be
 *  `structuredClone`d through Svelte proxies, so copy by hand; scalars pass
 *  through). List entries are `{ name: value }` objects — cloned field-by-field
 *  so a modal's scratch copy never aliases back into the shared schema. */
export function cloneParamValue<T extends FilterParamValue>(v: T): T {
    if (!Array.isArray(v)) return v;
    return v.map((x) => {
        if (Array.isArray(x)) return [...x];
        if (x && typeof x === 'object') {
            const out: Record<string, FilterParamValue> = {};
            for (const [k, val] of Object.entries(x)) out[k] = cloneParamValue(val as FilterParamValue);
            return out;
        }
        return x;
    }) as T;
}

/** True when a param has a meaningful neutral center to snap back to, and so
 *  earns a reset-to-default button: a 2D offset pad (recenters), or a numeric
 *  slider whose default sits in the *interior* of its range — a value you nudge
 *  away from in both directions, like scale's 1.0 between 0.9 and 1.1. Sliders
 *  whose default is a range endpoint (blur, which rests at 0) and deliberate
 *  picks (color) get none. */
export function paramIsResettable(param: FilterParam): boolean {
    if (param.kind === 'vec2') return true;
    if (param.kind === 'float' || param.kind === 'int') {
        const { min, max, default: d } = param;
        return (
            typeof min === 'number' &&
            typeof max === 'number' &&
            typeof d === 'number' &&
            d > min &&
            d < max
        );
    }
    return false;
}

/** The item schema of a `list` param — one `FilterParam` per entry field.
 *  Empty for non-list params (whose `options` is enum labels, not a schema). */
export function listItemSchema(param: FilterParam): FilterParam[] {
    if (param.kind !== 'list' || !Array.isArray(param.options)) return [];
    const opts = param.options as (string | FilterParam)[];
    return opts.every((o) => typeof o === 'object') ? (opts as FilterParam[]) : [];
}

/** Build a fresh list entry from an item schema: each field seeded to a
 *  deep-clone of its default. */
export function newListEntry(schema: FilterParam[]): Record<string, FilterParamValue> {
    const entry: Record<string, FilterParamValue> = {};
    for (const p of schema) entry[p.name] = cloneParamValue(p.default);
    return entry;
}

/** Seed an editable scratch copy of a schema's params for a modal: each param's
 *  `value` set to a deep-clone of its `default`. Editing the copy never touches
 *  the shared schema array. */
export function seedScratchParams(params: FilterParam[]): FilterParam[] {
    return params.map((p) => ({ ...p, value: cloneParamValue(p.default) }));
}

/** Build the `{ name: value }` map the engine's `updateFilterParams` /
 *  `applyFilter` expects — the effective value (`value ?? default`) per param. */
export function filterParamMap(params: FilterParam[]): Record<string, FilterParamValue> {
    const out: Record<string, FilterParamValue> = {};
    for (const p of params) out[p.name] = p.value ?? p.default;
    return out;
}

/** True when a `colorize` bool param is on. HSV's colorize overrides the model
 *  selector, so the editor disables the `model` enum while this holds. */
export function colorizeActive(params: FilterParam[]): boolean {
    const c = params.find((p) => p.name === 'colorize');
    return Boolean(c?.value ?? c?.default ?? false);
}

/**
 * Partition a filter's params into its per-channel tone params — each a channel
 * that gets its own entry in the channel selector — and its scalar params,
 * preserving declaration order within each group. A filter with N channel params
 * (Curves and Levels both expose rgb/red/green/blue/alpha/hue/saturation/
 * lightness) surfaces one selector with N options and a single per-channel
 * editor bound to the chosen channel.
 */
export function partitionFilterParams(params: FilterParam[]): {
    channels: FilterParam[];
    scalars: FilterParam[];
} {
    const channels: FilterParam[] = [];
    const scalars: FilterParam[] = [];
    for (const p of params) {
        (isChannelParam(p.kind) ? channels : scalars).push(p);
    }
    return { channels, scalars };
}

/** Channel ids that must render fully uppercase, not title-cased. */
const ACRONYM_CHANNELS = new Set(['rgb', 'rgba', 'cmyk', 'xyz', 'ycbcr']);

/**
 * Display label for a param name. Channel ids are lowercase stable ids
 * (`"rgb"`, `"saturation"`); acronyms render uppercase (`"rgb"` → `"RGB"`),
 * everything else title-cased (`"saturation"` → `"Saturation"`).
 */
export function channelLabel(name: string): string {
    if (ACRONYM_CHANNELS.has(name)) return name.toUpperCase();
    return name.charAt(0).toUpperCase() + name.slice(1);
}
