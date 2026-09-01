#!/usr/bin/env node
/**
 * Snapshot Google's own keyless font catalog into a trimmed, committed JSON the
 * Font Browser reads.
 *
 * Google's metadata endpoint (`https://fonts.google.com/metadata/fonts`) is
 * keyless and returns the full ~2.7 MB catalog, but it sends no
 * `Access-Control-Allow-Origin`, so a browser `fetch` is blocked. `curl`/`node`
 * aren't subject to CORS, so we snapshot at build time instead. The css2 byte
 * path the browser actually downloads fonts from *is* CORS-enabled; only this
 * catalog metadata needs the offline snapshot.
 *
 * Regenerate (a periodic PR, not a per-build step):
 *
 *     node scripts/update-font-catalog.mjs
 *
 * Writes `frontend/src/assets/google-fonts-catalog.json`, trimmed to the fields
 * the browser needs: family, category, axes (for the variable css2 URL), italic
 * availability (whether to fetch a second italic face), subsets, and popularity
 * (for default ordering).
 */
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = 'https://fonts.google.com/metadata/fonts';
const OUT = resolve(
    dirname(fileURLToPath(import.meta.url)),
    '../frontend/src/assets/google-fonts-catalog.json',
);

const res = await fetch(SRC);
if (!res.ok) {
    console.error(`Failed to fetch ${SRC}: ${res.status} ${res.statusText}`);
    process.exit(1);
}
const raw = await res.json();
const list = raw.familyMetadataList ?? [];

const trimmed = list.map((f) => ({
    family: f.family,
    category: f.category,
    // Only the axis tag + range matter for building a css2 variable URL.
    axes: (f.axes ?? []).map((a) => ({ tag: a.tag, min: a.min, max: a.max })),
    // `fonts` is a map keyed by style ("400", "400i", "700i", …); any `*i` key
    // means the family ships an italic face we can request via css2 `ital`.
    italic: Object.keys(f.fonts ?? {}).some((k) => k.endsWith('i')),
    subsets: f.subsets ?? [],
    popularity: f.popularity ?? Number.MAX_SAFE_INTEGER,
}));

// Popularity ascending = most popular first (Google ranks 1 = most popular).
trimmed.sort((a, b) => a.popularity - b.popularity);

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, JSON.stringify(trimmed));
console.log(`Wrote ${trimmed.length} families to ${OUT}`);
