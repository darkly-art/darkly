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

import type { ParamInfo, ParamValue } from '../../engine/protocol_gen';
import { cloneParamValue } from '../params/paramSchema';

export type { ParamInfo, ParamValue };

export type CurvePoints = [number, number][];
/** `[inBlack, inWhite, gamma, outBlack, outWhite]`: a Levels transfer. */
export type LevelsValues = [number, number, number, number, number];
/** A dynamic list of named-value entries: a `ParamValue::List`. */
export type ListValue = Record<string, ParamValue>[];

/** Kinds that are per-channel tone params, sharing the channel selector. */
export function isChannelParam(kind: string): boolean {
    return kind === 'curve' || kind === 'levels';
}

/** The item schema of a `list` param: one `ParamInfo` per entry field.
 *  Empty for non-list params (whose `options` is enum labels, not a schema). */
export function listItemSchema(param: ParamInfo): ParamInfo[] {
    if (param.kind !== 'list' || !Array.isArray(param.options)) return [];
    const opts = param.options as (string | ParamInfo)[];
    return opts.every((o) => typeof o === 'object') ? (opts as ParamInfo[]) : [];
}

/** Build a fresh list entry from an item schema: each field seeded to a
 *  deep-clone of its default. */
export function newListEntry(schema: ParamInfo[]): Record<string, ParamValue> {
    const entry: Record<string, ParamValue> = {};
    for (const p of schema) entry[p.name] = cloneParamValue(p.default);
    return entry;
}

/** Seed an editable scratch copy of a schema's params for a modal: each param's
 *  `value` set to a deep-clone of its `default`. Editing the copy never touches
 *  the shared schema array. */
export function seedScratchParams(params: ParamInfo[]): ParamInfo[] {
    return params.map((p) => ({ ...p, value: cloneParamValue(p.default) }));
}

/** Build the `{ name: value }` map the engine's `updateFilterParams` /
 *  `applyFilter` expects: the effective value (`value ?? default`) per param. */
export function filterParamMap(params: ParamInfo[]): Record<string, ParamValue> {
    const out: Record<string, ParamValue> = {};
    for (const p of params) out[p.name] = p.value ?? p.default;
    return out;
}

/** True when a `colorize` bool param is on. HSV's colorize overrides the model
 *  selector, so the editor disables the `model` enum while this holds. */
export function colorizeActive(params: ParamInfo[]): boolean {
    const c = params.find((p) => p.name === 'colorize');
    return Boolean(c?.value ?? c?.default ?? false);
}

/**
 * Partition a filter's params into its per-channel tone params (each a channel
 * that gets its own entry in the channel selector) and its scalar params,
 * preserving declaration order within each group. A filter with N channel params
 * (Curves and Levels both expose rgb/red/green/blue/alpha/hue/saturation/
 * lightness) surfaces one selector with N options and a single per-channel
 * editor bound to the chosen channel.
 */
export function partitionFilterParams(params: ParamInfo[]): {
    channels: ParamInfo[];
    scalars: ParamInfo[];
} {
    const channels: ParamInfo[] = [];
    const scalars: ParamInfo[] = [];
    for (const p of params) {
        (isChannelParam(p.kind) ? channels : scalars).push(p);
    }
    return { channels, scalars };
}
