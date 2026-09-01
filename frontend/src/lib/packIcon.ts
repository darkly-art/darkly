/**
 * Resolving a pack's icon to something that will actually draw.
 *
 * The icon bundle is generated offline by scraping Iconify name literals out of
 * the source (`frontend/scripts/gen-icon-bundle.mjs`), so a name that appears
 * only in an imported pack's manifest is not in it and would render nothing.
 * Rust validates a pack icon's *shape* (`collection:name`) and deliberately
 * stops there: whether an icon renders is the renderer's question, and the
 * renderer's answer is to fall back rather than show a hole.
 */
import { generateIcon } from '@iconify/svelte/dist/offline-functions.js';

/** Drawn in place of an icon the bundle lacks. Mirrors
 *  `PACK_ICON_FALLBACK` in `crates/darkly/src/brush/pack_icons.rs`, which is
 *  where the set a pack may choose from is declared. */
export const PACK_ICON_FALLBACK = 'fa6-solid:folder';

/** `name` if the offline bundle has it, the fallback otherwise. */
export function packIcon(name: string | null | undefined): string {
    if (name && generateIcon({ icon: name } as Parameters<typeof generateIcon>[0]) !== null) {
        return name;
    }
    return PACK_ICON_FALLBACK;
}
