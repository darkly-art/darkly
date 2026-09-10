#!/usr/bin/env node
/*
 * Resolve every Iconify name in the documentation metadata to actual SVG, and
 * write it beside the metadata as `icons.json`.
 *
 * This exists because an icon name is not a glyph. `metadata.json` says an
 * entry's icon is `fa6-solid:paintbrush`; turning that into artwork means owning
 * an icon toolchain and pinning the same icon-set versions this build used.
 * Resolving here follows the standing rule for this effort: where a consumer
 * would otherwise have to re-derive something, the producer stores it, and it
 * keeps the artifact's promise that reading it needs no particular renderer.
 *
 * It lives beside `gen-icon-bundle.mjs`, which does the same resolution for the
 * app's offline icon bundle (its throw-on-missing typo net is mirrored here),
 * and, being in `frontend/`, resolves `@iconify/json` and `@iconify/utils` from
 * the deps that already declare them. What that generator also does and this
 * deliberately does not is the resvg optical shrink-wrap: that matters for icons
 * sitting in a toolbar next to each other at 1em, not for table cells.
 *
 *   node frontend/scripts/export-doc-icons.mjs \
 *       --metadata out/metadata.json --out out/icons.json
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { getIconData } from '@iconify/utils';

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const JSON_DIR = path.join(FRONTEND, 'node_modules', '@iconify', 'json', 'json');

function parseArgs() {
    const args = process.argv.slice(2);
    const out = {};
    for (let i = 0; i < args.length; i += 2) {
        if (args[i] === '--metadata') out.metadata = args[i + 1];
        else if (args[i] === '--out') out.out = args[i + 1];
        else throw new Error(`unrecognized argument \`${args[i]}\``);
    }
    if (!out.metadata || !out.out) {
        throw new Error('usage: export-doc-icons.mjs --metadata <path> --out <path>');
    }
    return out;
}

/** Every icon name the metadata references, catalogs and entries alike. */
function iconNames(metadata) {
    const names = new Set();
    for (const catalog of metadata.catalogs ?? []) {
        if (catalog.icon) names.add(catalog.icon);
        for (const entry of catalog.entries ?? []) {
            if (entry.icon) names.add(entry.icon);
        }
    }
    return [...names].sort();
}

function main() {
    const { metadata: metaPath, out: outPath } = parseArgs();
    const metadata = JSON.parse(fs.readFileSync(metaPath, 'utf8'));
    const names = iconNames(metadata);

    if (!fs.existsSync(JSON_DIR)) {
        throw new Error(`@iconify/json is not installed: run \`npm ci\` in ${FRONTEND}`);
    }
    const loaded = new Map();
    const icons = {};
    const missing = [];

    for (const full of names) {
        const [prefix, name] = full.split(':');
        if (!loaded.has(prefix)) {
            const file = path.join(JSON_DIR, `${prefix}.json`);
            loaded.set(prefix, fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null);
        }
        const collection = loaded.get(prefix);
        // getIconData resolves aliases and applies the collection's default
        // dimensions, so callers get one uniform shape per icon.
        const data = collection && getIconData(collection, name);
        if (!data) {
            missing.push(full);
            continue;
        }
        icons[full] = {
            body: data.body,
            left: data.left ?? 0,
            top: data.top ?? 0,
            width: data.width ?? 16,
            height: data.height ?? 16,
        };
    }

    // Same hard typo net as the app's bundle: a name that resolves to nothing is
    // a bug in a registration, not something to paper over with a blank cell.
    if (missing.length) {
        throw new Error(`unresolvable icon name(s): ${missing.join(', ')}`);
    }

    fs.mkdirSync(path.dirname(path.resolve(outPath)), { recursive: true });
    fs.writeFileSync(outPath, `${JSON.stringify(icons, null, 0)}\n`);
    console.log(
        `${outPath}: ${names.length} icon(s) across ${new Set(names.map((n) => n.split(':')[0])).size} set(s)`,
    );
}

main();
