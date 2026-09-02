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

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPO = path.resolve(FRONTEND, '..');
const GRAPHICS_SRC = path.join(FRONTEND, 'src', 'graphics');

/** CANONICAL TWIN of `GRAPHICS_DIR` in
 *  `crates/darkly/src/docs_md/fragments/catalog_graphic.rs`, which emits the
 *  `<img src>` pointing here. A Rust const cannot be imported by a node script;
 *  if you move this, move that. */
const GRAPHICS_DIR = 'docs/images/graphics';

/** CANONICAL TWIN of `STILLS_DIR` in `crates/darkly/src/docs_md/mod.rs`, which
 *  is also where `render_docs --stills` writes. */
const STILLS_DIR = 'docs/images/previews';

/** Matches `write_jpeg` in `crates/darkly/src/docs_render/mod.rs`, and for the
 *  reason recorded there: below this, the pixelate veil's hard block edges ring. */
const JPEG_QUALITY = 90;

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
 * A context backed by the real metadata export and the committed stills.
 *
 * Both lookups throw by name. A graphic is a picture with no error state: a
 * missing still would render as a hole and get committed.
 */
export function diskContext(metadata, { stillsRoot = path.join(REPO, STILLS_DIR) } = {}) {
    return {
        catalog(id) {
            const found = (metadata.catalogs ?? []).find(c => c.id === id);
            if (!found) throw new Error(`no catalog \`${id}\` in the metadata export`);
            return found;
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
    };
}

/**
 * The component's markup and its scoped stylesheet, as one SVG string.
 *
 * Svelte's SSR output carries the scope class but not the CSS (`head` comes back
 * empty), so the stylesheet is compiled separately from the same source and
 * spliced in. The scope hash the compiler derives is identical to the one the
 * SSR module emitted, so the two halves line up without being coordinated.
 */
export function renderGraphic(component, source, filename, ssrRender, ctx) {
    const body = ssrRender(component.default, { props: component.graphicProps(ctx) }).body;
    const { css } = compile(source, { generate: 'server', filename });
    const svg = body.replace(/<!--\[-->|<!--\]-->/g, '');
    return css?.code ? svg.replace(/(<svg\b[^>]*>)/, `$1<style>${css.code}</style>`) : svg;
}

/**
 * The identity of an SVG, for deciding whether a committed image is stale.
 *
 * Two normalizations, both load-bearing:
 *
 * - Base64 stills become a hash of their bytes. The hash then covers *which*
 *   stills were used, without carrying half a megabyte of base64.
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

/**
 * What the committed image was rendered from, written beside it.
 *
 * `svg` is the gate: a test re-renders and compares it, which fails whenever the
 * component, its stylesheet, its layout constants or the stills' pixels change
 * without the image being regenerated.
 *
 * `entries` is what the test needs in order to re-render at all, since the
 * display names live in the Rust registries and reaching them requires a cargo
 * build that the frontend suite does not have. It is deliberately *not* the
 * catalog's authority: if a veil is added or renamed, this file goes stale and
 * says nothing, and the thing that notices is the generated alt text in
 * `README.md`, which comes straight from `catalogs()` and is checked by
 * `crates/darkly/tests/docs_md.rs`. Registry drift is Rust's gate; "the picture
 * matches its inputs" is this one's.
 */
function sidecar(component, ctx, svg, raster) {
    const catalog = ctx.catalog(component.catalog);
    return {
        svg: normalizedHash(svg),
        width: raster.width,
        height: raster.height,
        entries: catalog.entries.map(e => ({ type: e.type, displayName: e.displayName })),
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
            const svg = renderGraphic(component, source, file, render, ctx);
            const raster = rasterize(svg);

            const name = component.catalog;
            fs.writeFileSync(path.join(args.out, `${name}.jpg`), encodeJpeg(raster));
            fs.writeFileSync(
                path.join(args.out, `${name}.hash.json`),
                JSON.stringify(sidecar(component, ctx, svg, raster), null, 2) + '\n',
            );
            if (args.svg) fs.writeFileSync(path.join(args.svg, `${name}.svg`), svg);

            const bytes = fs.statSync(path.join(args.out, `${name}.jpg`)).size;
            console.log(
                `${name}: ${raster.width} x ${raster.height}, ${(bytes / 1024).toFixed(0)} KB`,
            );
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
