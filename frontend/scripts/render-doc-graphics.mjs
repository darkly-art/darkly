#!/usr/bin/env node
/*
 * Render every documentation graphic in `frontend/src/graphics/` to a committed
 * JPEG, plus a hash of what it was rendered from.
 *
 * A graphic is a Svelte component whose template is SVG. This exists because the
 * alternative was a markdown table: the registries already know every veil's
 * name and every veil already has a rendered still, but a table can only ever be
 * a table, and the README wanted a picture with the site's typography. So the
 * layout is authored the way the rest of the frontend is authored, and this
 * script is the part that turns it into pixels without a browser.
 *
 * A component declares what it depicts (`catalog`, optionally `category`) and
 * renders one image, or declares `instances(ctx)` and renders one image per
 * entry id it returns (a card per brush pack). Everything it draws it asks the
 * `GraphicContext` for by name; what it asked for is recorded beside the image
 * so the test can re-render without a cargo build.
 *
 * The pipeline, and why each step is where it is:
 *
 *   svelte/server render()   the component's markup, via a real Vite server so
 *                            the component resolves the same way it would in the
 *                            app
 *   svelte compile().css     the scoped stylesheet, which SSR does NOT emit and
 *                            which has to be spliced into the SVG by hand
 *   @resvg/resvg-js          rasterizes; already a devDependency, already driven
 *                            by gen-icon-bundle.mjs
 *   jpeg-js                  encodes; resvg only emits PNG, and PNG is ~6x the
 *                            bytes on a binary the repo re-commits on every veil
 *
 * resvg is not a browser. It has no flow layout and no text measurement, it
 * ignores `@font-face` and `var()`, and it renders `<foreignObject>` as nothing.
 * Fonts must be handed to it as file paths. Components are written against those
 * limits; see `frontend/src/graphics/Veils.svelte`.
 *
 *   cargo run -q -p darkly --bin export-docs -- --out target/docs/metadata.json
 *   node frontend/scripts/render-doc-graphics.mjs --metadata target/docs/metadata.json
 *
 * `--svg <dir>` also writes the intermediate SVG of each graphic, which is the
 * only way to see what resvg was actually handed.
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';

import { createServer } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { compile } from 'svelte/compiler';
import { Resvg } from '@resvg/resvg-js';
import jpeg from 'jpeg-js';

import { resolveIcon } from './doc-icons.mjs';

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPO = path.resolve(FRONTEND, '..');
const GRAPHICS_SRC = path.join(FRONTEND, 'src', 'graphics');

/** CANONICAL TWIN of `GRAPHICS_DIR` in
 *  `crates/darkly/src/docs_md/fragments/catalog_graphic.rs`, which emits the
 *  `<img src>` pointing here. A Rust const cannot be imported by a node script;
 *  if you move this, move that. */
const GRAPHICS_DIR = 'docs/images/graphics';

/** The file stem of a catalog graphic: the catalog id, plus the category when a
 *  graphic depicts one, or the entry when it depicts one, so one catalog can
 *  carry a picture per category or per entry without two of them claiming the
 *  same file. A category is a display label and is slugged; an entry is a type
 *  id, already a filename component, and is appended verbatim.
 *
 *  CANONICAL TWIN of `graphic_name` in
 *  `crates/darkly/src/docs_md/fragments/catalog_graphic.rs`, which links the
 *  file this writes. If you change one, change the other. */
export function graphicName(catalog, category, entry) {
    if (category) return `${catalog}-${category.replace(/[^a-zA-Z0-9]/g, '-').toLowerCase()}`;
    if (entry) return `${catalog}-${entry}`;
    return catalog;
}

/** Where an entry's optional hand-made banner lives:
 *  `docs/images/banners/<catalog>/<type>.jpg`. Rust never links a banner (it is
 *  an input to a picture, not a picture the markdown embeds), so unlike the two
 *  constants above this has no twin. */
const BANNERS_DIR = 'docs/images/banners';

/** CANONICAL TWIN of `STILLS_DIR` in `crates/darkly/src/docs_md/mod.rs`, which
 *  is also where `render_docs --stills` writes. */
const STILLS_DIR = 'docs/images/previews';

/** Matches `write_jpeg` in `crates/darkly/src/docs_render/mod.rs`, and for the
 *  reason recorded there: below this, the pixelate veil's hard block edges ring. */
const JPEG_QUALITY = 90;

/** The website's palette and type, spliced into every graphic ahead of the
 *  component's own scoped stylesheet. See the file for why it is unscoped. */
const SITE_CSS = fs.readFileSync(path.join(GRAPHICS_SRC, 'site.css'), 'utf8');

/** Every font a graphic may name, by path. resvg cannot read woff2 and ignores
 *  `@font-face`, so this list is the complete set of faces that exist as far as
 *  a graphic is concerned; a family name matching none of them falls back
 *  silently rather than erroring. See `src/graphics/fonts/NOTICE.md`. */
const FONT_FILES = [
    path.join(GRAPHICS_SRC, 'fonts', 'oldstyle-regular.ttf'),
    path.join(REPO, 'crates', 'darkly', 'resources', 'fonts', 'NotoSans-VF.ttf'),
];

function parseArgs(argv) {
    const out = {};
    for (let i = 0; i < argv.length; i += 2) {
        if (argv[i] === '--metadata') out.metadata = argv[i + 1];
        else if (argv[i] === '--out') out.out = argv[i + 1];
        else if (argv[i] === '--svg') out.svg = argv[i + 1];
        else throw new Error(`unrecognized argument \`${argv[i]}\``);
    }
    if (!out.metadata) {
        throw new Error(
            'usage: render-doc-graphics.mjs --metadata <metadata.json> [--out <dir>] [--svg <dir>]',
        );
    }
    out.out ??= path.join(REPO, GRAPHICS_DIR);
    return out;
}

/**
 * An export entry narrowed to what a graphic may read (`CatalogEntryView`).
 * Also exactly what a sidecar records, so the recording is a view of the
 * export and not a copy of it, and does not churn on fields no picture shows.
 */
function entryView(e) {
    const view = { type: e.type, displayName: e.displayName };
    if (e.description) view.description = e.description;
    if (e.icon) view.icon = e.icon;
    if (e.palette) view.palette = e.palette;
    if (e.members) view.members = e.members;
    return view;
}

/**
 * The banners on disk for every catalog in `known`, checked once.
 *
 * `known` maps a catalog id to the entry ids a banner may belong to. A banner
 * is declared by existing, so the scan is what makes that a safe authority: a
 * file under a known catalog whose stem is not one of its entries (a typo) is
 * refused by name, as is a directory that is not a known catalog, and a banner
 * that would be upscaled is refused when it is asked for at a width it cannot
 * fill. Anything other than `.jpg` is an orphan by the same rule.
 *
 * Returns the context's `banner(catalogId, typeId, slotWidth)`. The runner
 * calls this with the metadata's entries; the test calls it against the real
 * directory with the entries the committed sidecars record, which is the gate
 * that runs in CI.
 */
export function bannerLookup(bannersRoot, known) {
    const files = new Map();
    if (fs.existsSync(bannersRoot)) {
        for (const dir of fs.readdirSync(bannersRoot).sort()) {
            const catalogDir = path.join(bannersRoot, dir);
            if (!fs.statSync(catalogDir).isDirectory()) {
                throw new Error(`${catalogDir} is not a catalog directory; banners live in ${BANNERS_DIR}/<catalog>/`);
            }
            const entries = known.get(dir);
            if (!entries) throw new Error(`${catalogDir}: \`${dir}\` is not a catalog with per-entry graphics`);
            for (const file of fs.readdirSync(catalogDir).sort()) {
                const stem = file.replace(/\.jpg$/, '');
                if (stem === file || !entries.includes(stem)) {
                    throw new Error(
                        `${path.join(catalogDir, file)} is not a banner for any entry of \`${dir}\`; ` +
                            `expected one of: ${entries.map(e => `${e}.jpg`).join(', ')}`,
                    );
                }
                files.set(`${dir}/${stem}`, path.join(catalogDir, file));
            }
        }
    }
    return (catalogId, typeId, slotWidth) => {
        const file = files.get(`${catalogId}/${typeId}`);
        if (!file) return undefined;
        const bytes = fs.readFileSync(file);
        const { width } = jpeg.decode(bytes, { useTArray: true });
        if (width < slotWidth) {
            throw new Error(`${file} is ${width} px wide; the banner slot is ${slotWidth} px and it would be upscaled`);
        }
        return `data:image/jpeg;base64,${bytes.toString('base64')}`;
    };
}

/**
 * A context backed by the real metadata export and the committed stills.
 *
 * Every lookup but `banner` throws by name. A graphic is a picture with no
 * error state: a missing still would render as a hole and get committed.
 */
export function diskContext(
    metadata,
    { stillsRoot = path.join(REPO, STILLS_DIR), bannersRoot = path.join(REPO, BANNERS_DIR) } = {},
) {
    const catalogs = metadata.catalogs ?? [];
    const raw = id => {
        const found = catalogs.find(c => c.id === id);
        if (!found) throw new Error(`no catalog \`${id}\` in the metadata export`);
        return found;
    };
    const banner = bannerLookup(bannersRoot, new Map(catalogs.map(c => [c.id, c.entries.map(e => e.type)])));
    return {
        catalog(id, category) {
            const found = raw(id);
            if (!category) return { id, title: found.title, entries: found.entries.map(entryView) };
            const entries = found.entries.filter(e => e.category === category);
            if (!entries.length) {
                throw new Error(`catalog \`${id}\` has no entries in category \`${category}\``);
            }
            // The category names the narrowed view, matching the alt text the
            // Rust fragment generates for the same region.
            return { id, title: category, entries: entries.map(entryView) };
        },
        entry(catalogId, typeId) {
            const found = raw(catalogId).entries.find(e => e.type === typeId);
            if (!found) throw new Error(`catalog \`${catalogId}\` has no entry \`${typeId}\``);
            return entryView(found);
        },
        still(catalogId, typeId) {
            const file = path.join(stillsRoot, catalogId, `${typeId}.jpg`);
            if (!fs.existsSync(file)) {
                throw new Error(
                    `no still for ${catalogId}/${typeId} at ${path.relative(REPO, file)}\n` +
                        'render it first: cargo run --release -p darkly --features testing ' +
                        `--bin render_docs -- --stills --catalog ${catalogId}`,
                );
            }
            return `data:image/jpeg;base64,${fs.readFileSync(file).toString('base64')}`;
        },
        icon: iconLookup,
        banner,
    };
}

/** The context's `icon`: a glyph by name, or an error naming it. The same
 *  typo net `export-doc-icons.mjs` keeps. */
export function iconLookup(name) {
    const glyph = resolveIcon(name);
    if (!glyph) throw new Error(`no icon \`${name}\` in any installed icon set`);
    return glyph;
}

/**
 * A context that records what a component asked another context for.
 *
 * Only the catalog and entry answers are recorded, projected as views, so the
 * recording is proportional to the picture. Stills and banners are read from
 * disk again at test time and icons are resolved again from the installed icon
 * sets, so a change to any of them fails the hash as "stale, re-render" rather
 * than being replayed. `replayContext` is the inverse.
 */
export function recordingContext(ctx) {
    const record = { catalogs: {}, entries: {} };
    return {
        record,
        context: {
            catalog(id, category) {
                const view = ctx.catalog(id, category);
                record.catalogs[category ? `${id}#${category}` : id] = view;
                return view;
            },
            entry(catalogId, typeId) {
                const view = ctx.entry(catalogId, typeId);
                record.entries[`${catalogId}/${typeId}`] = view;
                return view;
            },
            still: (c, t) => ctx.still(c, t),
            icon: n => ctx.icon(n),
            banner: (c, t, w) => ctx.banner(c, t, w),
        },
    };
}

/** A context over a sidecar's recording plus the lookups that are not recorded. */
export function replayContext(record, { still, icon, banner }) {
    return {
        catalog(id, category) {
            const view = record.catalogs[category ? `${id}#${category}` : id];
            if (!view) throw new Error(`the recording has no catalog \`${id}\`${category ? ` in category \`${category}\`` : ''}`);
            return view;
        },
        entry(catalogId, typeId) {
            const view = record.entries[`${catalogId}/${typeId}`];
            if (!view) throw new Error(`the recording has no entry \`${catalogId}/${typeId}\``);
            return view;
        },
        still,
        icon,
        banner,
    };
}

/**
 * The component's markup and its stylesheets, as one SVG string.
 *
 * Svelte's SSR output carries the scope class but not the CSS (`head` comes back
 * empty), so the stylesheet is compiled separately from the same source and
 * spliced in. The scope hash the compiler derives is identical to the one the
 * SSR module emitted, so the two halves line up without being coordinated. The
 * shared `site.css` goes in ahead of it, unconditionally: a component with no
 * styles of its own still has a card to colour.
 *
 * `entry` is one of the component's `instances()`, or undefined for a component
 * that renders one image.
 */
export function renderGraphic(component, source, filename, ssrRender, ctx, entry) {
    const body = ssrRender(component.default, { props: component.graphicProps(ctx, entry) }).body;
    const { css } = compile(source, { generate: 'server', filename });
    const svg = body.replace(/<!--\[-->|<!--\]-->/g, '');
    return svg.replace(/(<svg\b[^>]*>)/, `$1<style>${SITE_CSS}${css?.code ?? ''}</style>`);
}

/**
 * The identity of an SVG, for deciding whether a committed image is stale.
 *
 * Two normalizations, both load-bearing:
 *
 * - Base64 JPEGs (stills and banners) become a hash of their bytes. The hash
 *   then covers *which* images were used, without carrying half a megabyte of
 *   base64.
 * - Svelte's scope class becomes a constant. It changes between Vite server
 *   instances with the component byte-identical on disk, and it reaches no
 *   pixel; left in, this hash would differ on every run.
 *
 * What survives is everything a component controls: geometry, labels, fonts,
 * palette, and the stills' content. No rasterizer runs, so it is identical on
 * every machine.
 */
export function normalizedHash(svg) {
    const normalized = svg
        .replace(
            /data:image\/jpeg;base64,([A-Za-z0-9+/=]+)/g,
            (_, b64) =>
                'still:' +
                crypto.createHash('sha256').update(Buffer.from(b64, 'base64')).digest('hex').slice(0, 16),
        )
        .replace(/svelte-[a-z0-9]{6,}/g, 'svelte-scope');
    return crypto.createHash('sha256').update(normalized).digest('hex').slice(0, 16);
}

/** Rasterize with the graphics font set. System fonts are off so that a machine
 *  with an unrelated face installed cannot change the output. */
export function rasterize(svg) {
    const image = new Resvg(svg, {
        logLevel: 'warn',
        font: { loadSystemFonts: false, fontFiles: FONT_FILES },
    }).render();
    return { rgba: Buffer.from(image.pixels), width: image.width, height: image.height };
}

export function encodeJpeg({ rgba, width, height }) {
    return jpeg.encode({ data: rgba, width, height }, JPEG_QUALITY).data;
}

export function decodeJpeg(bytes) {
    const { data, width, height } = jpeg.decode(bytes, { useTArray: true });
    return { rgba: data, width, height };
}

/**
 * The worst tile's RMSE between two rasters, 0 to 1.
 *
 * Tiled rather than whole-image on purpose. These graphics are mostly
 * photographic stills, which dominate the pixel count, so a real edit is a
 * rounding error globally: swapping the face on all ten veil labels moves 0.25%
 * of pixels, against 0.09% for JPEG round-trip noise alone. Taking the worst
 * tile localizes the change instead of averaging it away, which widens that
 * margin from 2.8x to 3.6x (0.101 against 0.028 on the same pair).
 */
export function worstTileRmse(a, b, cols = 8, rows = 6) {
    if (a.width !== b.width || a.height !== b.height) return 1;
    const tw = Math.ceil(a.width / cols);
    const th = Math.ceil(a.height / rows);
    let worst = 0;
    for (let ty = 0; ty < rows; ty++) {
        for (let tx = 0; tx < cols; tx++) {
            let sum = 0;
            let n = 0;
            const yEnd = Math.min((ty + 1) * th, a.height);
            const xEnd = Math.min((tx + 1) * tw, a.width);
            for (let y = ty * th; y < yEnd; y++) {
                for (let x = tx * tw; x < xEnd; x++) {
                    const i = (y * a.width + x) * 4;
                    for (let c = 0; c < 3; c++) {
                        const d = a.rgba[i + c] - b.rgba[i + c];
                        sum += d * d;
                        n++;
                    }
                }
            }
            if (n) worst = Math.max(worst, Math.sqrt(sum / n) / 255);
        }
    }
    return worst;
}

/**
 * What the committed image was rendered from, written beside it.
 *
 * `svg` is the gate: a test re-renders and compares it, which fails whenever the
 * component, its stylesheet, its layout constants or the stills' pixels change
 * without the image being regenerated.
 *
 * `context` is what the test needs in order to re-render at all: the catalog
 * and entry views the component asked for, since the display names live in the
 * Rust registries and reaching them requires a cargo build that the frontend
 * suite does not have. It is deliberately *not* the catalog's authority: if a
 * veil is added or renamed, this file goes stale and says nothing, and the
 * thing that notices is the generated alt text in `README.md`, which comes
 * straight from `catalogs()` and is checked by `crates/darkly/tests/docs_md.rs`.
 * Registry drift is Rust's gate; "the picture matches its inputs" is this one's.
 *
 * `catalog`, `category` and `entry` say which component and which of its
 * instances the file belongs to, so the test can pair them without parsing
 * the file name.
 */
function sidecar(component, entry, svg, raster, record) {
    return {
        svg: normalizedHash(svg),
        width: raster.width,
        height: raster.height,
        catalog: component.catalog,
        category: component.category ?? null,
        entry: entry ?? null,
        context: record,
    };
}

/** Every graphic component, by the catalog it depicts. */
function graphicFiles() {
    return fs
        .readdirSync(GRAPHICS_SRC)
        .filter(f => f.endsWith('.svelte'))
        .sort()
        .map(f => path.join(GRAPHICS_SRC, f));
}

/**
 * Load every graphic through a real Vite server, the way the app loads a
 * component.
 *
 * The test calls this too, rather than reaching for `import.meta.glob`, and that
 * is not incidental: two Vite pipelines configured differently emit different
 * markup for the same component (this project's own config and a bare
 * `svelte()` disagree about whether the root element carries the scope class),
 * so a test that compiled its own copy would be checking a picture nothing ever
 * renders. One loader, one answer.
 *
 * `render` has to come from `ssrLoadModule` rather than a bare import, or the
 * component and the renderer end up with two copies of Svelte's server
 * internals and `render()` throws on a null context.
 */
export async function openGraphics() {
    const server = await createServer({
        configFile: false,
        root: FRONTEND,
        appType: 'custom',
        logLevel: 'warn',
        optimizeDeps: { noDiscovery: true },
        resolve: { conditions: ['browser'] },
        server: { middlewareMode: true, fs: { allow: [REPO] } },
        plugins: [svelte()],
    });
    try {
        const { render } = await server.ssrLoadModule('svelte/server');
        const graphics = [];
        for (const file of graphicFiles()) {
            graphics.push({
                file,
                component: await server.ssrLoadModule(file),
                source: fs.readFileSync(file, 'utf8'),
            });
        }
        return { graphics, render, close: () => server.close() };
    } catch (e) {
        await server.close();
        throw e;
    }
}

async function main() {
    const args = parseArgs(process.argv.slice(2));
    const metadata = JSON.parse(fs.readFileSync(args.metadata, 'utf8'));
    const ctx = diskContext(metadata);

    const { graphics, render, close } = await openGraphics();

    try {
        fs.mkdirSync(args.out, { recursive: true });
        if (args.svg) fs.mkdirSync(args.svg, { recursive: true });

        for (const { file, component, source } of graphics) {
            // A picture is of a group or of an entry, never both; the Rust
            // fragment refuses the same combination.
            if (component.category && component.instances) {
                throw new Error(`${file} declares both a category and instances`);
            }
            for (const entry of component.instances?.(ctx) ?? [undefined]) {
                const { record, context } = recordingContext(ctx);
                const svg = renderGraphic(component, source, file, render, context, entry);
                const raster = rasterize(svg);

                const name = graphicName(component.catalog, component.category, entry);
                fs.writeFileSync(path.join(args.out, `${name}.jpg`), encodeJpeg(raster));
                fs.writeFileSync(
                    path.join(args.out, `${name}.hash.json`),
                    JSON.stringify(sidecar(component, entry, svg, raster, record), null, 2) + '\n',
                );
                if (args.svg) fs.writeFileSync(path.join(args.svg, `${name}.svg`), svg);

                const bytes = fs.statSync(path.join(args.out, `${name}.jpg`)).size;
                console.log(
                    `${name}: ${raster.width} x ${raster.height}, ${(bytes / 1024).toFixed(0)} KB`,
                );
            }
        }
    } finally {
        await close();
    }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    main().catch(e => {
        console.error(`render-doc-graphics: ${e.message}`);
        process.exit(1);
    });
}
