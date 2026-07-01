/**
 * Pure helpers for the filter-layer properties panel.
 *
 * A filter's params (the `ParamInfo[]` emitted by the Rust `filter_types()` /
 * layer-tree query) mix scalar controls (sliders, checkboxes) with tone curves.
 * The panel renders every curve param through one shared channel selector +
 * `<CurveEditor>`; the scalars render as their own rows. This module owns the
 * split so the component stays declarative and the grouping is unit-testable.
 */

export type CurvePoints = [number, number][];

/** One entry of a filter's `params` array — a `ParamInfo` view. */
export interface FilterParam {
    kind: string;
    name: string;
    min?: number;
    max?: number;
    default: number | boolean | CurvePoints;
    value?: number | boolean | CurvePoints;
}

/**
 * Partition a filter's params into its curve params — each a channel that gets
 * its own entry in the channel selector — and its scalar params, preserving
 * declaration order within each group. A filter with N curve params (curves
 * exposes red/green/blue/value/alpha) surfaces one selector with N options and
 * a single curve editor bound to the chosen channel.
 */
export function partitionFilterParams(params: FilterParam[]): {
    curves: FilterParam[];
    scalars: FilterParam[];
} {
    const curves: FilterParam[] = [];
    const scalars: FilterParam[] = [];
    for (const p of params) {
        (p.kind === 'curve' ? curves : scalars).push(p);
    }
    return { curves, scalars };
}
