import { describe, it, expect, beforeAll, afterAll } from 'vitest';
// @ts-ignore: Node builtins; the project intentionally omits @types/node (see
// vite.config.ts). Vitest runs under node, so these resolve at runtime.
import { readFileSync, existsSync, readdirSync, mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
// @ts-ignore: as above.
import path from 'node:path';
// @ts-ignore: as above.
import os from 'node:os';
// @ts-ignore: as above.
import { fileURLToPath } from 'node:url';
// @ts-ignore: a native napi module whose types reference node globals the
// frontend tsconfig deliberately lacks.
import { Resvg } from '@resvg/resvg-js';
// Typed by scripts/render-doc-graphics.d.mts, hand-written so this import is
// checked rather than arriving as `any`. The runner is .mjs and lives outside
// src/ for the resvg-types reason above.
import {
    openGraphics,
    renderGraphic,
    normalizedHash,
    rasterize,
    decodeJpeg,
    encodeJpeg,
    worstTileRmse,
    graphicName,
    bannerLookup,
    iconLookup,
    replayContext,
    type ContextRecord,
    type LoadedGraphic,
} from '../../../scripts/render-doc-graphics.mjs';
import type { GraphicContext } from '../context';
import type { PackPalette } from '../../engine/protocol_gen';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../..');
const GRAPHICS = path.join(REPO, 'docs/images/graphics');
const STILLS = path.join(REPO, 'docs/images/previews');
const BANNERS = path.join(REPO, 'docs/images/banners');
const OLDSTYLE = path.join(REPO, 'frontend/src/graphics/fonts/oldstyle-regular.ttf');
const NOTO = path.join(REPO, 'crates/darkly/resources/fonts/NotoSans-VF.ttf');

/** The card colour `site.css` declares. */
const CARD = [0x0b, 0x0a, 0x0d];

interface Sidecar {
    svg: string;
    width: number;
    height: number;
    catalog: string;
    category: string | null;
    entry: string | null;
    context: ContextRecord;
}

interface Size {
    width: number;
    height: number;
}

/** The graphics, loaded through the runner's own Vite server. Deliberately not
 *  `import.meta.glob`: this project's Vite config and the runner's disagree
 *  about whether the root element carries the scope class, so a locally
 *  compiled copy would not be the picture the runner writes. */
let loaded: LoadedGraphic[];
let render: (...args: unknown[]) => { body: string };
let close: () => Promise<void>;

beforeAll(async () => {
    ({ graphics: loaded, render, close } = await openGraphics());
}, 60_000);

afterAll(async () => {
    await close?.();
});

function staleMessage(name: string): string {
    return (
        `${name}.jpg is stale; re-render it:\n` +
        '  cargo run -q -p darkly --bin export-docs -- --out target/docs/metadata.json\n' +
        '  node frontend/scripts/render-doc-graphics.mjs --metadata target/docs/metadata.json'
    );
}

/** Every committed sidecar, by the file stem it shares with its image. */
function sidecars(): { name: string; sidecar: Sidecar }[] {
    return readdirSync(GRAPHICS)
        .filter((f: string) => f.endsWith('.hash.json'))
        .sort()
        .map((f: string) => ({
            name: f.replace(/\.hash\.json$/, ''),
            sidecar: JSON.parse(readFileSync(path.join(GRAPHICS, f), 'utf8')) as Sidecar,
        }));
}

/** The component a sidecar was rendered from: the one depicting the same
 *  catalog and category. */
function graphicFor(s: Sidecar): LoadedGraphic {
    const found = loaded.find(
        g => g.component.catalog === s.catalog && ((g.component.category as string | undefined) ?? null) === s.category,
    );
    if (!found) throw new Error(`no graphic depicts \`${s.catalog}\`${s.category ? ` / ${s.category}` : ''}`);
    return found;
}

/** Every (component, sidecar) pair, which is what every per-image assertion
 *  iterates: one component may own several images. */
function pairs(): { g: LoadedGraphic; name: string; sidecar: Sidecar }[] {
    return sidecars().map(({ name, sidecar }) => ({ g: graphicFor(sidecar), name, sidecar }));
}

function stillUri(catalogId: string, typeId: string): string {
    const file = path.join(STILLS, catalogId, `${typeId}.jpg`);
    if (!existsSync(file)) {
        throw new Error(`no still for ${catalogId}/${typeId} at ${path.relative(REPO, file)}`);
    }
    return `data:image/jpeg;base64,${readFileSync(file).toString('base64')}`;
}

/** Which entries may own a committed banner: exactly the per-entry images on
 *  disk. A banner for anything else is a typo or a leftover, and the lookup
 *  refuses it, which is how a misnamed banner fails this suite rather than
 *  silently rendering as "no banner". */
function knownBannerOwners(): Map<string, string[]> {
    const known = new Map<string, string[]>();
    for (const { sidecar } of sidecars()) {
        if (sidecar.entry === null) continue;
        known.set(sidecar.catalog, [...(known.get(sidecar.catalog) ?? []), sidecar.entry]);
    }
    return known;
}

/** A context over exactly what the committed image was rendered from: the
 *  recorded views, the real committed stills and banners, and the installed
 *  icon sets. */
function committedContext(s: Sidecar): GraphicContext {
    return replayContext(s.context, {
        still: stillUri,
        icon: iconLookup,
        banner: bannerLookup(BANNERS, knownBannerOwners()),
    });
}

function svgFor(g: LoadedGraphic, s: Sidecar, ctx = committedContext(s)): string {
    return renderGraphic(g.component, g.source, g.file, render, ctx, s.entry ?? undefined);
}

/** One re-render per committed image for the whole suite. Three tests read the
 *  same fresh picture, and rasterizing six cards three times over was enough
 *  to push the suite past its timeout under a loaded CI runner. */
const fresh = new Map<string, { svg: string; raster: ReturnType<typeof rasterize> }>();
function freshFor(g: LoadedGraphic, name: string, s: Sidecar) {
    let got = fresh.get(name);
    if (!got) {
        const svg = svgFor(g, s);
        got = { svg, raster: rasterize(svg) };
        fresh.set(name, got);
    }
    return got;
}

/** Rasterizing every card is slow; these tests are given room beyond the
 *  default so a loaded machine fails on a wrong picture, not on the clock. */
const RENDER_TIMEOUT = 60_000;

function sizeOf(g: LoadedGraphic, ctx: GraphicContext, entry?: string): Size {
    const props = (g.component.graphicProps as (c: GraphicContext, e?: string) => unknown)(ctx, entry);
    return (g.component.size as (p: unknown) => Size)(props);
}

/** Pixels brighter than a threshold. Deliberately NOT an alpha count: the card
 *  is opaque `#0b0a0d`, so "has alpha" is true of a solid black rectangle and
 *  would pass for a graphic that drew nothing at all. */
function litFraction(rgba: Uint8Array, threshold = 40): number {
    let n = 0;
    for (let i = 0; i < rgba.length; i += 4) {
        if (0.2126 * rgba[i] + 0.7152 * rgba[i + 1] + 0.0722 * rgba[i + 2] > threshold) n++;
    }
    return n / (rgba.length / 4);
}

function pixelAt(rgba: Uint8Array, width: number, x: number, y: number): number[] {
    const i = (y * width + x) * 4;
    return [rgba[i], rgba[i + 1], rgba[i + 2]];
}

function hex(c: string): number[] {
    return [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16));
}

function distance(a: number[], b: number[]): number {
    return Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
}

/** The bounding box of everything lit, which pins a face, its size and its
 *  position in one tuple. */
function inkBounds(rgba: Uint8Array, width: number, height: number, threshold = 40) {
    let minX = width;
    let minY = height;
    let maxX = -1;
    let maxY = -1;
    for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
            const i = (y * width + x) * 4;
            if (0.2126 * rgba[i] + 0.7152 * rgba[i + 1] + 0.0722 * rgba[i + 2] > threshold) {
                if (x < minX) minX = x;
                if (x > maxX) maxX = x;
                if (y < minY) minY = y;
                if (y > maxY) maxY = y;
            }
        }
    }
    return maxX < 0 ? null : { x: minX, y: minY, w: maxX - minX + 1, h: maxY - minY + 1 };
}

/** "Veils" alone, so the assertion is about the glyphs and not the card. */
function renderTitle(family: string) {
    const svg =
        '<svg xmlns="http://www.w3.org/2000/svg" width="500" height="140">' +
        `<text x="10" y="100" font-family="${family}" font-size="72" fill="#ffffff">Veils</text></svg>`;
    const image = new Resvg(svg, {
        logLevel: 'error',
        font: { loadSystemFonts: false, fontFiles: [OLDSTYLE, NOTO] },
    }).render();
    return { rgba: Uint8Array.from(image.pixels), width: image.width, height: image.height };
}

/** A flat JPEG as a data URI, for stills and banners the fixtures invent. */
function flatJpeg(width: number, height: number, rgb: number[]): Uint8Array {
    const rgba = new Uint8Array(width * height * 4);
    for (let i = 0; i < rgba.length; i += 4) {
        rgba[i] = rgb[0];
        rgba[i + 1] = rgb[1];
        rgba[i + 2] = rgb[2];
        rgba[i + 3] = 255;
    }
    return encodeJpeg({ rgba, width, height });
}

function dataUri(jpeg: Uint8Array): string {
    // @ts-ignore: Node global, for the reason given on the imports above.
    return `data:image/jpeg;base64,${Buffer.from(jpeg).toString('base64')}`;
}

/** The constants and helpers `BrushPack.svelte` exports from its module
 *  script, which the fixtures read back rather than restating. */
interface PackCard {
    W: number;
    BANNER_H: number;
    OVERLAP: number;
    PAD: number;
    HEADER_H: number;
    STILL_W: number;
    STILL_H: number;
    TINT: number;
    INK: string;
    mix(a: string, b: string, t: number): string;
}

describe('documentation graphics', () => {
    it('discovers at least one graphic', () => {
        expect(loaded.length).toBeGreaterThan(0);
    });

    it('every graphic declares the contract the runner calls', () => {
        for (const g of loaded) {
            expect(typeof g.component.catalog, g.file).toBe('string');
            expect(typeof g.component.graphicProps, g.file).toBe('function');
            expect(typeof g.component.size, g.file).toBe('function');
            expect(typeof g.component.default, g.file).toBe('function');
        }
    });

    it('every graphic owns at least one committed image, and every image a graphic', () => {
        const all = sidecars();
        for (const g of loaded) {
            const own = all.filter(
                ({ sidecar }) =>
                    sidecar.catalog === g.component.catalog &&
                    sidecar.category === ((g.component.category as string | undefined) ?? null),
            );
            expect(own.length, `${g.file} has no committed image`).toBeGreaterThan(0);
        }
        for (const { name, sidecar } of all) {
            expect(() => graphicFor(sidecar), name).not.toThrow();
            expect(name).toBe(graphicName(sidecar.catalog, sidecar.category, sidecar.entry));
        }
    });

    it('every graphic renders a real picture', () => {
        for (const { g, name, sidecar } of pairs()) {
            const ctx = committedContext(sidecar);
            const { raster } = freshFor(g, name, sidecar);

            // Dimensions come from the component's own exported constants
            // rather than being restated here, so a layout change fails this.
            expect({ width: raster.width, height: raster.height }, name).toEqual(
                sizeOf(g, ctx, sidecar.entry ?? undefined),
            );

            // Exactly the declared hex, which holds only if `site.css` was
            // spliced and applied; without it the card is black. Sampled in
            // the bottom pad, which is card colour whether or not a banner
            // fills the top of the card, and off the rounded corner, which
            // is genuinely transparent.
            expect(pixelAt(raster.rgba, raster.width, raster.width >> 1, raster.height - 3), name).toEqual(CARD);

            // Something was actually drawn. A black rectangle scores ~0 here;
            // the sparsest real card (one brush on a dark surface, the pack
            // with a single member) scores about 0.02.
            expect(litFraction(raster.rgba), name).toBeGreaterThan(0.01);
        }
    }, RENDER_TIMEOUT);

    it('sets the title in Oldstyle, at the size the stylesheet asks for', () => {
        const title = renderTitle('OldStyle 1');
        const box = inkBounds(title.rgba, title.width, title.height);
        expect(box).not.toBeNull();

        // A recorded box pins face, size and position together, so this fails
        // on the wrong typeface, the wrong size, or nothing drawn. The window is
        // wide enough for rasterizer differences and far too narrow for another
        // face at another size.
        expect(box!.w).toBeGreaterThan(130);
        expect(box!.w).toBeLessThan(210);
        expect(box!.h).toBeGreaterThan(40);
        expect(box!.h).toBeLessThan(80);

        // A different loaded face must give a different box. Without this the
        // assertion above would pass for any font that happened to be loaded:
        // resvg falls back silently on a family name it cannot match, so a
        // misspelled family renders plausibly and tests green.
        const other = renderTitle('Noto Sans');
        const otherBox = inkBounds(other.rgba, other.width, other.height)!;
        expect([otherBox.w, otherBox.h]).not.toEqual([box!.w, box!.h]);
    });

    it('treats a missing still as an error, not a hole', () => {
        expect(() => stillUri('veils', 'no_such_veil')).toThrow(/no_such_veil/);
    });

    it('treats an unresolvable icon as an error, not a hole', () => {
        // Assembled at runtime: `gen-icon-bundle.mjs` scrapes `collection:name`
        // literals out of the sources into the app's offline bundle, and a
        // name that exists only to be missing must not end up there.
        const missing = ['mdi', 'no-such-glyph'].join(':');
        expect(() => iconLookup(missing)).toThrow(/no-such-glyph/);
    });

    it('every committed image is up to date with its component and stills', () => {
        for (const { g, name, sidecar } of pairs()) {
            const { svg, raster } = freshFor(g, name, sidecar);

            expect({ width: raster.width, height: raster.height }, name).toEqual({
                width: sidecar.width,
                height: sidecar.height,
            });
            expect(normalizedHash(svg), staleMessage(name)).toBe(sidecar.svg);
        }
    }, RENDER_TIMEOUT);

    // The hash above attests to the SVG, not to the file on disk, so on its own
    // the committed JPEG could be anything at all. This is the only assertion
    // that reads the artifact the README actually embeds.
    //
    // Fuzzy because it must survive JPEG requantization and any rasterizer
    // difference between machines, neither of which is a stale image. Measured
    // headroom on this composition: round-trip noise scores 0.028 and a real
    // edit (the label font on all ten veils) scores 0.101, so the threshold sits
    // between them with roughly 2x margin on each side. If this ever flakes,
    // raise the threshold rather than deleting the check.
    it.each([0.06])('every committed image matches its pixels (worst-tile RMSE < %s)', limit => {
        for (const { g, name, sidecar } of pairs()) {
            const file = path.join(GRAPHICS, `${name}.jpg`);
            expect(existsSync(file), `${file} is missing`).toBe(true);

            const committed = decodeJpeg(readFileSync(file));
            const { raster } = freshFor(g, name, sidecar);

            expect({ width: committed.width, height: committed.height }, name).toEqual({
                width: raster.width,
                height: raster.height,
            });
            expect(worstTileRmse(committed, raster), staleMessage(name)).toBeLessThan(limit);
        }
    }, RENDER_TIMEOUT);
});

describe('banners', () => {
    // The declaring act for a banner is placing a file, so these are the
    // "declared but wrong" cases, exercised through the function the suite's
    // own committed-context gate uses.
    const known = new Map([['brushPacks', ['dry_media']]]);

    function bannersDir(files: Record<string, Uint8Array>): string {
        const root = mkdtempSync(path.join(os.tmpdir(), 'darkly-banners-'));
        for (const [rel, bytes] of Object.entries(files)) {
            mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
            writeFileSync(path.join(root, rel), bytes);
        }
        return root;
    }

    it('the committed banners all belong to entries with graphics', () => {
        expect(() => bannerLookup(BANNERS, knownBannerOwners())).not.toThrow();
    });

    it('an absent banner is an answer, not an error', () => {
        const banner = bannerLookup(bannersDir({}), known);
        expect(banner('brushPacks', 'dry_media', 1444)).toBeUndefined();
    });

    it('a banner for no entry is refused by name', () => {
        const root = bannersDir({ 'brushPacks/dry-media.jpg': flatJpeg(8, 8, [0, 0, 0]) });
        expect(() => bannerLookup(root, known)).toThrow(/dry-media\.jpg/);
    });

    it('a banner under a catalog with no per-entry graphics is refused', () => {
        const root = bannersDir({ 'effects/frozen.jpg': flatJpeg(8, 8, [0, 0, 0]) });
        expect(() => bannerLookup(root, known)).toThrow(/effects/);
    });

    it('a banner narrower than its slot is refused, naming both widths', () => {
        const root = bannersDir({ 'brushPacks/dry_media.jpg': flatJpeg(200, 60, [0, 0, 0]) });
        const banner = bannerLookup(root, known);
        expect(() => banner('brushPacks', 'dry_media', 1444)).toThrow(/200 px.*1444 px/);
    });
});

describe('the brush pack card', () => {
    const palette: PackPalette = { chroma: '#c2521f', refraction: '#2fd0c0', surface: '#b4491a1a' };

    /** A one-pack, two-brush world: all-white stills, so a tile is the tint
     *  gradient and nothing else, and the real `mdi:pencil` glyph. */
    function fixture(banner?: string): GraphicContext {
        const still = dataUri(flatJpeg(320, 120, [255, 255, 255]));
        return replayContext(
            {
                catalogs: {},
                entries: {
                    'brushPacks/fixture': {
                        type: 'fixture',
                        displayName: 'Fixture',
                        description: 'A pack for the test.',
                        icon: 'mdi:pencil',
                        palette,
                        members: { catalog: 'brushes', entries: ['a', 'b'] },
                    },
                    'brushes/a': { type: 'a', displayName: 'Alpha' },
                    'brushes/b': { type: 'b', displayName: 'Beta' },
                },
            },
            { still: () => still, icon: iconLookup, banner: () => banner },
        );
    }

    function card() {
        const g = loaded.find(x => x.component.catalog === 'brushPacks');
        if (!g) throw new Error('no graphic depicts brushPacks');
        return g;
    }

    function renderCard(ctx: GraphicContext) {
        const g = card();
        return { g, raster: rasterize(renderGraphic(g.component, g.source, g.file, render, ctx, 'fixture')) };
    }

    it('renders without a banner, with the strokes tinted by the pair', () => {
        const ctx = fixture();
        const { g, raster } = renderCard(ctx);
        const c = g.component as unknown as PackCard;
        expect({ width: raster.width, height: raster.height }).toEqual(sizeOf(g, ctx, 'fixture'));
        expect(litFraction(raster.rgba)).toBeGreaterThan(0.01);

        // The first tile is a white still under the ink gradient, so its
        // centre is the gradient's midpoint at TINT, and its edges lean to
        // chroma on the left and refraction on the right. The midpoint alone
        // would pass with the two swapped.
        const start = c.mix(palette.chroma, c.INK, c.TINT);
        const end = c.mix(palette.refraction, c.INK, c.TINT);
        const mid = hex(c.mix(start, end, 0.5));
        const cy = c.PAD + c.HEADER_H + (c.STILL_H >> 1);
        const centre = pixelAt(raster.rgba, raster.width, c.PAD + (c.STILL_W >> 1), cy);
        expect(distance(centre, mid), `centre ${centre} vs ${mid}`).toBeLessThan(8);
        const left = pixelAt(raster.rgba, raster.width, c.PAD + 6, cy);
        const right = pixelAt(raster.rgba, raster.width, c.PAD + c.STILL_W - 7, cy);
        expect(distance(left, hex(start))).toBeLessThan(distance(left, hex(end)));
        expect(distance(right, hex(end))).toBeLessThan(distance(right, hex(start)));
    });

    it('a banner makes the card taller by the band and shows through at the top', () => {
        const without = renderCard(fixture()).raster;
        const bannerRgb = [40, 200, 90];
        const c = card().component as unknown as PackCard;
        const { raster } = renderCard(fixture(dataUri(flatJpeg(c.W, c.BANNER_H, bannerRgb))));

        expect(raster.height - without.height).toBe(c.BANNER_H - c.OVERLAP - c.PAD);
        // Above the fade the banner is drawn as is.
        expect(distance(pixelAt(raster.rgba, raster.width, raster.width >> 1, 40), bannerRgb)).toBeLessThan(6);
        // And the bottom pad is still the card, which is what the shared
        // card-pixel assertion relies on.
        expect(pixelAt(raster.rgba, raster.width, raster.width >> 1, raster.height - 3)).toEqual(CARD);
    });
});
