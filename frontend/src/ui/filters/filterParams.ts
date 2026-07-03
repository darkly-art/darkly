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

/** One entry of a filter's `params` array — a `ParamInfo` view. */
export interface FilterParam {
    kind: string;
    name: string;
    min?: number;
    max?: number;
    default: number | boolean | CurvePoints | LevelsValues;
    value?: number | boolean | CurvePoints | LevelsValues;
}

/** Kinds that are per-channel tone params, sharing the channel selector. */
export function isChannelParam(kind: string): boolean {
    return kind === 'curve' || kind === 'levels';
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
