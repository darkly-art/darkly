/*
 * Resolve an Iconify name to a glyph, from the icon sets installed under
 * `node_modules/@iconify/json`.
 *
 * One resolver for the three places a documentation artifact turns an icon
 * name into artwork: `export-doc-icons.mjs` (which writes `icons.json` beside
 * `metadata.json`), the graphics runner (which draws a glyph into an SVG) and
 * the graphics test (which re-renders what the runner wrote). They have to
 * agree on what a name means, and sharing the lookup is how they do.
 *
 * `null` on a miss rather than a throw: what a miss means is the caller's call.
 * The exporter and the runner both treat it as a bug in a registration, the
 * same hard typo net `gen-icon-bundle.mjs` keeps for the app's bundle.
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { getIconData } from '@iconify/utils';

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const JSON_DIR = path.join(FRONTEND, 'node_modules', '@iconify', 'json', 'json');

/** Parsed icon sets by prefix, `null` for a prefix no installed set has. */
const collections = new Map();

/**
 * `{ body, left, top, width, height }` for `collection:name`, or `null` when
 * no installed set has it. `getIconData` resolves aliases and applies the
 * set's default dimensions, so every glyph comes back in one shape.
 */
export function resolveIcon(full) {
    if (!fs.existsSync(JSON_DIR)) {
        throw new Error(`@iconify/json is not installed: run \`npm ci\` in ${FRONTEND}`);
    }
    const [prefix, name] = full.split(':');
    if (!collections.has(prefix)) {
        const file = path.join(JSON_DIR, `${prefix}.json`);
        collections.set(prefix, fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null);
    }
    const collection = collections.get(prefix);
    const data = collection && getIconData(collection, name);
    if (!data) return null;
    return {
        body: data.body,
        left: data.left ?? 0,
        top: data.top ?? 0,
        width: data.width ?? 16,
        height: data.height ?? 16,
    };
}
