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

/** A concrete param value on the wire (matches `ParamValue` in `protocol_gen`). */
export type FilterParamValue = number | boolean | CurvePoints | LevelsValues;

/** One entry of a filter's `params` array — a `ParamInfo` view. */
export interface FilterParam {
    kind: string;
    name: string;
    min?: number;
    max?: number;
    default: FilterParamValue;
    value?: FilterParamValue;
    /** Enum only: the option labels (stored value is the index into this). */
    options?: string[];
}

/** Kinds that are per-channel tone params, sharing the channel selector. */
export function isChannelParam(kind: string): boolean {
    return kind === 'curve' || kind === 'levels';
}

/** Deep-clone a param value (curve pairs / levels arrays can't be `structuredClone`d
 *  through Svelte proxies, so copy arrays by hand; scalars pass through). */
export function cloneParamValue<T extends FilterParamValue>(v: T): T {
    return (Array.isArray(v) ? v.map((x) => (Array.isArray(x) ? [...x] : x)) : v) as T;
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
