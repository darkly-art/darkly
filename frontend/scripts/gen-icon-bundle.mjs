// Icon-bundle generator. Scans the frontend source AND the Rust crate for
// Iconify icon-name string literals ("prefix:name") and emits
// src/icons/bundle.generated.ts — a module that registers exactly those icons
// (and only those) for OFFLINE rendering. No central hand-maintained list:
// adding an icon anywhere in the source is purely additive, the same way
// build.rs auto-discovers Rust modules.
//
// This module exposes two entry points so generation is owned by the build,
// not a fragile manual pre-step:
//   - `iconBundlePlugin()` — a Vite plugin that regenerates on buildStart (dev
//     AND prod) and re-runs whenever a scanned source file changes during
//     `vite dev`, so editing an icon name updates the bundle live (HMR).
//   - the CLI (`node scripts/gen-icon-bundle.mjs`, i.e. `npm run gen:icons`) —
//     for one-off regeneration and the `pretest` hook (Vitest doesn't run the
//     plugin's buildStart).
//
// A prefix is treated as an icon set when @iconify/json ships a collection for
// it, plus the synthetic `local` set sourced from src/icons/svg/*.svg. The
// generator THROWS if a referenced name is absent from its collection — the
// hard typo safety net that replaces Font Awesome's silent fallback. (A typo'd
// *prefix* isn't a known set, so it's skipped here and caught instead by the
// dev-runtime warning in Icon.svelte + the markup test in iconBundle.test.ts.)

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { getIcons } from '@iconify/utils';

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SRC = path.join(FRONTEND, 'src');
const SVG_DIR = path.join(SRC, 'icons', 'svg');
const JSON_DIR = path.join(FRONTEND, 'node_modules', '@iconify', 'json', 'json');
const OUT = path.join(SRC, 'icons', 'bundle.generated.ts');
// The Rust crate also names icons (settings-section tabs, brush-node icon
// pickers) that cross the WASM boundary and render in the UI — scan it too.
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

function buildLocal(names) {
    const icons = {};
    for (const name of names) {
        const f = path.join(SVG_DIR, `${name}.svg`);
        if (!fs.existsSync(f)) {
            throw new Error(`[gen-icons] local:${name} referenced but ${path.relative(FRONTEND, f)} is missing`);
        }
        const svg = fs.readFileSync(f, 'utf8');
        const vb = svg.match(/viewBox\s*=\s*"\s*([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s*"/);
        const width = vb ? Number(vb[3]) : 16;
        const height = vb ? Number(vb[4]) : 16;
        // Inner body kept verbatim (defs / gradients / ids preserved) — Iconify
        // stores it as-is (no SVGO), and uniquifies ids at render time.
        const body = svg
            .replace(/^[\s\S]*?<svg[^>]*>/, '')
            .replace(/<\/svg>\s*$/, '')
            .trim();
        icons[name] = { body, width, height };
    }
    return { prefix: 'local', icons };
}

/** Scan the source roots and produce the bundle module text. Throws on an
 *  unknown icon name within a known set. */
function renderBundle() {
    const byPrefix = new Map(); // prefix -> Set(name)
    for (const file of ROOTS.flatMap((r) => walk(r))) {
        const text = fs.readFileSync(file, 'utf8');
        for (const m of text.matchAll(NAME_RE)) {
            const [, prefix, name] = m;
            if (prefix !== 'local' && !collectionExists(prefix)) continue; // not an icon set
            if (!byPrefix.has(prefix)) byPrefix.set(prefix, new Set());
            byPrefix.get(prefix).add(name);
        }
    }

    const collections = [];
    let total = 0;
    for (const [prefix, set] of [...byPrefix].sort(([a], [b]) => a.localeCompare(b))) {
        const names = [...set].sort();
        total += names.length;
        if (prefix === 'local') {
            collections.push(buildLocal(names));
            continue;
        }
        const full = JSON.parse(fs.readFileSync(path.join(JSON_DIR, `${prefix}.json`), 'utf8'));
        const subset = getIcons(full, names);
        if (!subset) throw new Error(`[gen-icons] failed to read collection "${prefix}"`);
        if (subset.not_found?.length) {
            throw new Error(
                `[gen-icons] unknown icon(s) in "${prefix}": ${subset.not_found.join(', ')} — fix the name or pick a valid id`,
            );
        }
        collections.push(subset);
    }

    const banner = `// AUTO-GENERATED by scripts/gen-icon-bundle.mjs — DO NOT EDIT BY HAND.
// Regenerated automatically by the icon-bundle Vite plugin (dev + build) and by
// \`npm run gen:icons\`. Derived from the Iconify icon-name string literals found
// in the source, registered for offline rendering.
// ${total} icon(s) across ${collections.length} collection(s).
`;
    // NB: the `.js` extension is load-bearing — it routes through the package's
    // `./*` catch-all export to the real file. The bare `./dist/offline-functions`
    // subpath is declared in the package's exports map but points at a path that
    // doesn't ship, so it fails to resolve.
    let out = `${banner}/* eslint-disable */\n// @ts-nocheck\nimport { addCollection } from '@iconify/svelte/dist/offline-functions.js';\n\n`;
    for (const data of collections) out += `addCollection(${JSON.stringify(data)});\n`;

    // The full set of names that resolve offline — the source of truth for the
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
                logger.info(`[icons] ${r.changed ? 'generated' : 'up to date'} — ${r.total} icon(s), ${r.collections} collection(s)`);
            } catch (e) {
                // Fail the production build; in dev, log and let the watcher
                // recover once the offending name is fixed.
                if (isBuild) throw e;
                logger.error(`[icons] ${e.message}`);
            }
        },
        configureServer(server) {
            // The Rust crate lives outside Vite's root — watch it explicitly.
            server.watcher.add(CRATE_SRC);
            const onChange = (file) => {
                if (!isScanned(file)) return;
                try {
                    const r = generateIconBundle();
                    if (r.changed) logger.info(`[icons] regenerated — ${r.total} icon(s), ${r.collections} collection(s)`);
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
    console.log(`[gen-icons] ${r.changed ? 'wrote' : 'unchanged'} ${r.path} — ${r.total} icon(s), ${r.collections} collection(s)`);
}
