/**
 * Keyless Google Fonts import + decode.
 *
 * No API key at build or runtime. The catalog is a committed build-time snapshot
 * (`assets/google-fonts-catalog.json`, produced by `scripts/update-font-catalog.mjs`),
 * lazily imported so it stays out of the main bundle. The byte path is the
 * keyless, CORS-enabled css2 endpoint: we ask `fonts.googleapis.com/css2` for a
 * family, the browser receives woff2 files from `fonts.gstatic.com`, and we
 * decode woff2 → SFNT on the frontend with `wawoff2` before handing raw TTF to
 * the engine via the personal font library.
 *
 * The Phase-0 spike proved parley honors a weight scrub against a variable face,
 * so we request a single variable file per family (`css2?family=X:wght@min..max`)
 * — one blob covers every weight.
 *
 * Google splits a family's woff2 by unicode subset (one file per script). The
 * engine maps one family → one blob, so we import the **Latin** subset (Darkly's
 * primary script); non-Latin scripts are a documented follow-up.
 */
import { fontLibrary } from '../state/font_library.svelte';

export interface CatalogAxis {
    tag: string;
    min: number;
    max: number;
}

export interface CatalogFont {
    family: string;
    category: string;
    axes: CatalogAxis[];
    subsets: string[];
    popularity: number;
}

const CSS2 = 'https://fonts.googleapis.com/css2';

let catalogPromise: Promise<CatalogFont[]> | null = null;

/** Lazily load the committed catalog snapshot. Same-origin import (no CORS),
 *  deferred so the hundreds-of-KB JSON only loads when the browser opens. */
export function loadCatalog(): Promise<CatalogFont[]> {
    if (!catalogPromise) {
        catalogPromise = import('../assets/google-fonts-catalog.json').then(
            (m) => m.default as CatalogFont[],
        );
    }
    return catalogPromise;
}

/** Build a keyless css2 URL for `family`. Requests the full `wght` range as a
 *  single variable file when the family has a `wght` axis (per the spike);
 *  otherwise the static family. `italic` adds the `ital` axis if present;
 *  `text` subsets the download to just those characters (used for name
 *  previews — the browser loads it natively, no decode). */
export function cssUrl(
    family: string,
    axes: CatalogAxis[],
    opts: { italic?: boolean; text?: string } = {},
): string {
    const name = family.trim().replace(/\s+/g, '+');
    const wght = axes.find((a) => a.tag === 'wght');
    const hasItal = opts.italic && axes.some((a) => a.tag === 'ital');

    let spec = name;
    if (wght && hasItal) {
        spec = `${name}:ital,wght@0,${wght.min}..${wght.max};1,${wght.min}..${wght.max}`;
    } else if (wght) {
        spec = `${name}:wght@${wght.min}..${wght.max}`;
    } else if (hasItal) {
        spec = `${name}:ital@1`;
    }

    let url = `${CSS2}?family=${spec}&display=swap`;
    if (opts.text) url += `&text=${encodeURIComponent(opts.text)}`;
    return url;
}

/** Every `url(...)` in a css2 response, in document order. */
function extractFontUrls(css: string): string[] {
    const urls: string[] = [];
    const re = /url\((https:\/\/[^)]+)\)/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(css)) !== null) urls.push(m[1]);
    return urls;
}

/** Pick the Latin subset block's font URL from a css2 response — the `@font-face`
 *  whose `unicode-range` covers basic Latin (`U+0000-00FF`). Falls back to the
 *  last URL (Google orders Latin last) when ranges can't be parsed. */
function pickLatinUrl(css: string): string | null {
    const blocks = css.split('@font-face');
    for (const b of blocks) {
        const url = b.match(/url\((https:\/\/[^)]+)\)/);
        if (!url) continue;
        const range = b.match(/unicode-range:\s*([^;]+);/);
        if (range && /U\+0000-00FF/i.test(range[1])) return url[1];
    }
    const all = extractFontUrls(css);
    return all.length ? all[all.length - 1] : null;
}

/** Decode a woff2 blob to raw SFNT (TTF) bytes. `wawoff2` is a WASM module,
 *  dynamically imported so it stays out of the main bundle until a font is
 *  actually imported. */
async function decodeWoff2(woff2: Uint8Array): Promise<Uint8Array> {
    const { decompress } = await import('wawoff2');
    return await decompress(woff2);
}

/**
 * Import a Google font into the personal library: fetch its css2, decode the
 * Latin woff2 to TTF, and register it. Returns the family names the decoded font
 * contributed (empty on failure). Idempotent via the library's content-hash
 * dedup — importing the same font twice costs nothing.
 */
export async function importFont(font: CatalogFont): Promise<string[]> {
    const css = await (await fetch(cssUrl(font.family, font.axes))).text();
    const url = pickLatinUrl(css);
    if (!url) {
        console.warn(`[fonts] no font URL in css2 response for ${font.family}`);
        return [];
    }
    const woff2 = new Uint8Array(await (await fetch(url)).arrayBuffer());
    const ttf = await decodeWoff2(woff2);
    return await fontLibrary.add(ttf, 'google');
}
