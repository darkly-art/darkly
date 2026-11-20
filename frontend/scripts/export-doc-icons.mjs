#!/usr/bin/env node
/*
 * Resolve every Iconify name in the documentation metadata to actual SVG, and
 * write it beside the metadata as `icons.json`.
 *
 * This exists because an icon name is not a glyph. `metadata.json` says an
 * entry's icon is `fa6-solid:paintbrush`; turning that into artwork means owning
 * an icon toolchain and pinning the same icon-set versions this build used.
 * Resolving here keeps the docs site free of an icon toolchain of its own: it
 * reads resolved SVG from the export, the same way it reads everything else.
 *
 * The lookup itself is `doc-icons.mjs`, shared with the graphics runner, which
 * draws the same glyphs into its SVGs. Both keep the throw-on-missing typo net
 * of `gen-icon-bundle.mjs`, the app's offline bundle generator. What that
 * generator also does and this deliberately does not is the resvg optical
 * shrink-wrap: that matters for icons sitting in a toolbar next to each other
 * at 1em, not for table cells.
 *
 *   node frontend/scripts/export-doc-icons.mjs \
 *       --metadata out/metadata.json --out out/icons.json
 */

import fs from 'node:fs';
import path from 'node:path';
import { resolveIcon } from './doc-icons.mjs';

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

    const icons = {};
    const missing = [];
    for (const full of names) {
        const glyph = resolveIcon(full);
        if (glyph) icons[full] = glyph;
        else missing.push(full);
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
