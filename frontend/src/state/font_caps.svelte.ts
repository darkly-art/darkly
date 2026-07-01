/**
 * Font capabilities cache — what a family can do (variable axes + a real italic
 * face), fetched from the engine's `font_axes` request and cached per family so
 * the text-properties panel can render font-driven controls without refetching
 * on every keystroke.
 *
 * The cache is invalidated whenever the personal font library changes: a
 * same-named re-import may carry different bytes (hence different axes), and
 * `fontLibrary.families` is replaced with a fresh array on every genuine change,
 * so comparing its identity is a cheap, correct invalidation token.
 */
import { fontLibrary } from './font_library.svelte';
import type { Engine } from '../engine/protocol';

/** One variable-font axis a family exposes, with the font's real range. */
export interface AxisInfo {
    tag: string;
    min: number;
    default: number;
    max: number;
}

/** A family's capabilities: whether it has a real italic face, and its axes. */
export interface FontCapabilities {
    italic: boolean;
    axes: AxisInfo[];
}

const cache = new Map<string, FontCapabilities>();
/** Identity of the `fontLibrary.families` array the cache was populated under;
 *  a change means the library was updated, so the cache is stale. */
let cacheToken: string[] | null = null;

/** Fetch (and cache) `family`'s capabilities. Reads `fontLibrary.families` so a
 *  caller inside a Svelte `$effect` re-runs when the library changes; the read
 *  also drives cache invalidation. */
export async function fontCaps(engine: Engine, family: string): Promise<FontCapabilities> {
    if (cacheToken !== fontLibrary.families) {
        cache.clear();
        cacheToken = fontLibrary.families;
    }
    const hit = cache.get(family);
    if (hit) return hit;
    const res = (await engine
        .send<FontCapabilities>('font_axes', { family })
        .catch(() => null)) as FontCapabilities | null;
    const caps: FontCapabilities = { italic: !!res?.italic, axes: res?.axes ?? [] };
    cache.set(family, caps);
    return caps;
}
