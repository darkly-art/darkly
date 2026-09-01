// Icon-bundle generator. Scans the frontend source AND the Rust crate for
// Iconify icon-name string literals ("prefix:name") and emits
// src/icons/bundle.generated.ts: a module that registers exactly those icons
// (and only those) for OFFLINE rendering. No central hand-maintained list:
// adding an icon anywhere in the source is purely additive, the same way
// build.rs auto-discovers Rust modules.
//
// This module exposes two entry points so generation is owned by the build,
// not a fragile manual pre-step:
//   - `iconBundlePlugin()` - a Vite plugin that regenerates on buildStart (dev
//     AND prod) and re-runs whenever a scanned source file changes during
//     `vite dev`, so editing an icon name updates the bundle live (HMR).
//   - the CLI (`node scripts/gen-icon-bundle.mjs`, i.e. `npm run gen:icons`) -
//     for one-off regeneration and the `pretest` hook (Vitest doesn't run the
//     plugin's buildStart).
//
// A prefix is treated as an icon set when @iconify/json ships a collection for
// it. There is no bespoke-SVG escape hatch: every icon Darkly names comes from a
// published set, which is what lets any consumer of Darkly's metadata resolve an
// icon name without this repo's help. The
// generator THROWS if a referenced name is absent from its collection: the
// hard typo safety net that replaces Font Awesome's silent fallback. (A typo'd
// *prefix* isn't a known set, so it's skipped here and caught instead by the
// dev-runtime warning in Icon.svelte + the markup test in iconBundle.test.ts.)

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { getIcons } from '@iconify/utils';
import { Resvg } from '@resvg/resvg-js';

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SRC = path.join(FRONTEND, 'src');
const JSON_DIR = path.join(FRONTEND, 'node_modules', '@iconify', 'json', 'json');
const OUT = path.join(SRC, 'icons', 'bundle.generated.ts');
// The Rust crate also names icons (settings-section tabs, brush-node icon
// pickers) that cross the WASM boundary and render in the UI; scan it too.
const CRATE_SRC = path.join(FRONTEND, '..', 'crates', 'darkly', 'src');
const ROOTS = [SRC, CRATE_SRC];
const SCAN_RE = /\.(ts|svelte|rs)$/;

// "prefix:name" inside a quote/backtick. Both segments are kebab tokens.
const NAME_RE = /['"`]([a-z][a-z0-9]*(?:-[a-z0-9]+)*):([a-z0-9]+(?:-[a-z0-9]+)*)['"`]/g;

const collectionExists = (prefix) => fs.existsSync(path.join(JSON_DIR, `${prefix}.json`));

function walk(dir, acc = []) {
    if (!fs.existsSync(dir)) return acc;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const p = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            if (entry.name !== 'node_modules' && entry.name !== 'target') walk(p, acc);
        } else if (SCAN_RE.test(entry.name) && !p.endsWith('bundle.generated.ts')) {
            acc.push(p);
        }
    }
    return acc;
}

// Icons ship as inline SVGs forced to a 1em square (see Icon.svelte +
// ToolCluster's `.tool svg` rule). An icon's on-screen size is therefore how
// much of its viewBox the artwork covers, and icon sets bake in wildly
// different margins (Font Awesome solids touch all four edges; Boxicons/Lucide
// dashed marquees sit inside a 4-12% margin). The result: select-tool icons
// render visibly smaller than the fill/eyedropper tools beside them.
//
// We normalize optical size by shrink-wrapping every icon's viewBox to its
// inked bounds at BUILD time. The measurement rasterizes each icon and reads
// the alpha bounding box (exact for curves and strokes, no geometry math); the
// SHIPPED icon stays a vector: only its `viewBox`/`left/top/width/height` are
// rewritten to hug the artwork. resvg is a devDependency; nothing renders at
// runtime. Bodies are identical across runs, so results are memoized to keep
// dev/HMR regeneration cheap.
const MEASURE_PX = 512; // render size on the icon's longest axis
const tightBoxCache = new Map(); // body -> [left, top, width, height]
const round3 = (n) => Math.round(n * 1000) / 1000;

/** Rasterize one icon and return the tight [left, top, width, height] of its
 *  inked pixels, expressed back in the icon's own viewBox units. Returns the
 *  input box unchanged if the icon renders empty. */
function tightBox(body, left, top, width, height) {
    const cached = tightBoxCache.get(body);
    if (cached) return cached;

    const ppu = MEASURE_PX / Math.max(width, height); // pixels per viewBox unit
    const pxW = Math.round(width * ppu);
    const pxH = Math.round(height * ppu);
    // currentColor has no resolution context here; pin it opaque so both
    // fills and strokes register in the alpha channel we scan.
    const painted = body.replaceAll('currentColor', '#000');
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${left} ${top} ${width} ${height}" width="${pxW}" height="${pxH}">${painted}</svg>`;

    const img = new Resvg(svg).render();
    const { width: w, height: h, pixels } = img;
    let minX = w, minY = h, maxX = -1, maxY = -1;
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            if (pixels[(y * w + x) * 4 + 3] > 0) {
                if (x < minX) minX = x;
                if (x > maxX) maxX = x;
                if (y < minY) minY = y;
                if (y > maxY) maxY = y;
            }
        }
    }
    let box;
    if (maxX < 0) {
        box = [left, top, width, height]; // nothing inked; leave as-is
    } else {
        // Grow the pixel box by 1px each side so anti-aliased stroke edges are
        // never clipped, then clamp to the render and map px → viewBox units.
        minX = Math.max(0, minX - 1);
        minY = Math.max(0, minY - 1);
        maxX = Math.min(w - 1, maxX + 1);
        maxY = Math.min(h - 1, maxY + 1);
        box = [
            round3(left + minX / ppu),
            round3(top + minY / ppu),
            round3((maxX - minX + 1) / ppu),
            round3((maxY - minY + 1) / ppu),
        ];
    }
    tightBoxCache.set(body, box);
    return box;
}

/** Shrink-wrap every icon in a collection to its inked bounds, writing explicit
 *  left/top/width/height onto each icon (overriding the collection defaults). */
function tightenCollection(data) {
    const dw = data.width ?? 16;
    const dh = data.height ?? 16;
    for (const icon of Object.values(data.icons)) {
        const [l, t, w, h] = tightBox(
            icon.body,
            icon.left ?? 0,
            icon.top ?? 0,
            icon.width ?? dw,
            icon.height ?? dh,
        );
        icon.left = l;
        icon.top = t;
        icon.width = w;
        icon.height = h;
    }
    return data;
}

/** Scan the source roots and produce the bundle module text. Throws on an
 *  unknown icon name within a known set. */
function renderBundle() {
    const byPrefix = new Map(); // prefix -> Set(name)
    for (const file of ROOTS.flatMap((r) => walk(r))) {
        const text = fs.readFileSync(file, 'utf8');
        for (const m of text.matchAll(NAME_RE)) {
            const [, prefix, name] = m;
            if (!collectionExists(prefix)) continue; // not an icon set
            if (!byPrefix.has(prefix)) byPrefix.set(prefix, new Set());
            byPrefix.get(prefix).add(name);
        }
    }

    const collections = [];
    let total = 0;
    for (const [prefix, set] of [...byPrefix].sort(([a], [b]) => a.localeCompare(b))) {
        const names = [...set].sort();
        total += names.length;
        const full = JSON.parse(fs.readFileSync(path.join(JSON_DIR, `${prefix}.json`), 'utf8'));
        const subset = getIcons(full, names);
        if (!subset) throw new Error(`[gen-icons] failed to read collection "${prefix}"`);
        if (subset.not_found?.length) {
            throw new Error(
                `[gen-icons] unknown icon(s) in "${prefix}": ${subset.not_found.join(', ')}, fix the name or pick a valid id`,
            );
        }
        collections.push(tightenCollection(subset));
    }

    const banner = `// AUTO-GENERATED by scripts/gen-icon-bundle.mjs: DO NOT EDIT BY HAND.
// Regenerated automatically by the icon-bundle Vite plugin (dev + build) and by
// \`npm run gen:icons\`. Derived from the Iconify icon-name string literals found
// in the source, registered for offline rendering.
// ${total} icon(s) across ${collections.length} collection(s).
`;
    // NB: the `.js` extension is load-bearing: it routes through the package's
    // `./*` catch-all export to the real file. The bare `./dist/offline-functions`
    // subpath is declared in the package's exports map but points at a path that
    // doesn't ship, so it fails to resolve.
    let out = `${banner}/* eslint-disable */\n// @ts-nocheck\nimport { addCollection } from '@iconify/svelte/dist/offline-functions.js';\n\n`;
    for (const data of collections) out += `addCollection(${JSON.stringify(data)});\n`;

    // The full set of names that resolve offline: the source of truth for the
    // curated icon picker (e.g. custom brush-bar entries). Sorted for stable diffs.
    const bundledNames = [...byPrefix.entries()]
        .flatMap(([prefix, set]) => [...set].map((n) => `${prefix}:${n}`))
        .sort();
    out += `\nexport const BUNDLED_ICON_NAMES = ${JSON.stringify(bundledNames)};\n`;

    return { out, total, collections: collections.length };
}

/** Regenerate the bundle, writing only when the content actually changed (so we
 *  don't trigger needless HMR churn). Returns a small status object. */
export function generateIconBundle() {
    const { out, total, collections } = renderBundle();
    const prev = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf8') : null;
    const changed = prev !== out;
    if (changed) {
        fs.mkdirSync(path.dirname(OUT), { recursive: true });
        fs.writeFileSync(OUT, out);
    }
    return { total, collections, changed, path: path.relative(FRONTEND, OUT) };
}

/** Vite plugin: own icon-bundle generation as part of the build. */
export function iconBundlePlugin() {
    let isBuild = false;
    let logger = console;
    const isScanned = (file) => SCAN_RE.test(file)
        && !file.endsWith('bundle.generated.ts')
        && ROOTS.some((root) => file.startsWith(root + path.sep));

    return {
        name: 'darkly-icon-bundle',
        configResolved(cfg) {
            isBuild = cfg.command === 'build';
            logger = cfg.logger ?? console;
        },
        buildStart() {
            try {
                const r = generateIconBundle();
                logger.info(`[icons] ${r.changed ? 'generated' : 'up to date'}: ${r.total} icon(s), ${r.collections} collection(s)`);
            } catch (e) {
                // Fail the production build; in dev, log and let the watcher
                // recover once the offending name is fixed.
                if (isBuild) throw e;
                logger.error(`[icons] ${e.message}`);
            }
        },
        configureServer(server) {
            // The Rust crate lives outside Vite's root; watch it explicitly.
            server.watcher.add(CRATE_SRC);
            const onChange = (file) => {
                if (!isScanned(file)) return;
                try {
                    const r = generateIconBundle();
                    if (r.changed) logger.info(`[icons] regenerated: ${r.total} icon(s), ${r.collections} collection(s)`);
                } catch (e) {
                    logger.error(`[icons] ${e.message}`);
                    server.ws.send({ type: 'error', err: { message: e.message, stack: '', plugin: 'darkly-icon-bundle' } });
                }
            };
            server.watcher.on('change', onChange);
            server.watcher.on('add', onChange);
            server.watcher.on('unlink', onChange);
        },
    };
}

// CLI entry: `node scripts/gen-icon-bundle.mjs` / `npm run gen:icons`.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    const r = generateIconBundle();
    console.log(`[gen-icons] ${r.changed ? 'wrote' : 'unchanged'} ${r.path}: ${r.total} icon(s), ${r.collections} collection(s)`);
}
