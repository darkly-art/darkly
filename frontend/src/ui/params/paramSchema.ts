/**
 * The param vocabulary shared by every panel that renders a `ParamInfo[]`.
 *
 * Filters, veils and voids all carry the same wire type (`ParamInfo`, see
 * `engine/protocol_gen.ts`), so the value aliases, the deep clone and the
 * label rules belong to none of them in particular. This module is named
 * after the shape it describes rather than after the first panel that needed
 * it.
 *
 * Filter-shaped logic (the channel split, list entries, scratch seeding)
 * stays in `ui/filters/filterParams.ts`, which imports from here.
 */

import type { ParamInfo, ParamValue } from '../../engine/protocol_gen';

// The wire types are authoritative and are re-exported rather than restated.
// (The hand-written union that used to live here had already drifted: it
// omitted `string`, which the blender void's `url` param carries. A panel-local
// alias for `ParamValue` is how that drift started, so there is not one here.)
export type { ParamInfo, ParamValue };

/** Normalized sRGB `[r, g, b]` in `[0,1]`: a `ParamValue::Color`. */
export type ColorValue = [number, number, number];
/** A 2D vector `[x, y]`: a `ParamValue::Vec2` (offset pad). */
export type Vec2Value = [number, number];

/** Deep-clone a param value (curve pairs / levels arrays / list entries can't be
 *  `structuredClone`d through Svelte proxies, so copy by hand; scalars pass
 *  through). List entries are `{ name: value }` objects, cloned field-by-field
 *  so a modal's scratch copy never aliases back into the shared schema. */
export function cloneParamValue<T extends ParamValue>(v: T): T {
    if (!Array.isArray(v)) return v;
    return v.map((x) => {
        if (Array.isArray(x)) return [...x];
        if (x && typeof x === 'object') {
            const out: Record<string, ParamValue> = {};
            for (const [k, val] of Object.entries(x)) out[k] = cloneParamValue(val as ParamValue);
            return out;
        }
        return x;
    }) as T;
}

/** True when a param has a meaningful neutral center to snap back to, and so
 *  earns a reset-to-default button: a 2D offset pad (recenters), or a numeric
 *  slider whose default sits in the *interior* of its range: a value you nudge
 *  away from in both directions, like scale's 1.0 between 0.9 and 1.1. Sliders
 *  whose default is a range endpoint (blur, which rests at 0) and deliberate
 *  picks (color) get none. */
export function paramIsResettable(param: ParamInfo): boolean {
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

