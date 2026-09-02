import { describe, it, expect, beforeAll, afterAll } from 'vitest';
// @ts-ignore: Node builtins; the project intentionally omits @types/node (see
// vite.config.ts). Vitest runs under node, so these resolve at runtime.
import { readFileSync, existsSync } from 'node:fs';
// @ts-ignore: as above.
import path from 'node:path';
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
} from '../../../scripts/render-doc-graphics.mjs';
import type { GraphicContext } from '../context';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../..');
const GRAPHICS = path.join(REPO, 'docs/images/graphics');
const STILLS = path.join(REPO, 'docs/images/previews');
const OLDSTYLE = path.join(REPO, 'frontend/src/graphics/fonts/oldstyle-regular.ttf');
const NOTO = path.join(REPO, 'crates/darkly/resources/fonts/NotoSans-VF.ttf');

interface Sidecar {
    svg: string;
    width: number;
    height: number;
    entries: { type: string; displayName: string }[];
}

interface Loaded {
    file: string;
    component: Record<string, unknown>;
    source: string;
}

/** The graphics, loaded through the runner's own Vite server. Deliberately not
 *  `import.meta.glob`: this project's Vite config and the runner's disagree
 *  about whether the root element carries the scope class, so a locally
 *  compiled copy would not be the picture the runner writes. */
let loaded: Loaded[];
let render: (...args: unknown[]) => { body: string };
let close: () => Promise<void>;

beforeAll(async () => {
    ({ graphics: loaded, render, close } = await openGraphics());
}, 60_000);

afterAll(async () => {
    await close?.();
});

function sidecarFor(catalog: string): Sidecar {
    return JSON.parse(readFileSync(path.join(GRAPHICS, `${catalog}.hash.json`), 'utf8'));
}

function stillUri(catalogId: string, typeId: string): string {
    const file = path.join(STILLS, catalogId, `${typeId}.jpg`);
    if (!existsSync(file)) {
        throw new Error(`no still for ${catalogId}/${typeId} at ${path.relative(REPO, file)}`);
    }
    return `data:image/jpeg;base64,${readFileSync(file).toString('base64')}`;
}

/** A context over exactly what the committed image was rendered from: the
 *  recorded entries and the real committed stills. */
function committedContext(catalog: string, title: string): GraphicContext {
    const { entries } = sidecarFor(catalog);
    return { catalog: (id: string) => ({ id, title, entries }), still: stillUri };
}

/** Titles come from the Rust registries, which need a cargo build the frontend
 *  suite does not have. The catalog id is enough for these assertions. */
const TITLES: Record<string, string> = { veils: 'Veils' };

function svgFor(g: Loaded): string {
    const catalog = g.component.catalog as string;
    return renderGraphic(
        g.component,
        g.source,
        g.file,
        render,
        committedContext(catalog, TITLES[catalog] ?? catalog),
    );
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

describe('documentation graphics', () => {
    it('discovers at least one graphic', () => {
        expect(loaded.length).toBeGreaterThan(0);
    });

    it('every graphic declares the contract the runner calls', () => {
        for (const g of loaded) {
            expect(typeof g.component.catalog, g.file).toBe('string');
            expect(typeof g.component.graphicProps, g.file).toBe('function');
            expect(typeof g.component.default, g.file).toBe('function');
        }
    });

    it('every graphic renders a real picture', () => {
        for (const g of loaded) {
            const catalog = g.component.catalog as string;
            const raster = rasterize(svgFor(g));

            // Dimensions come from the component's own exported constants
            // rather than being restated here, so a layout change fails this.
            const count = sidecarFor(catalog).entries.length;
            expect({ width: raster.width, height: raster.height }, g.file).toEqual(
                (g.component.size as (n: number) => unknown)(count),
            );

            // Exactly the declared hex, which holds only if the scoped <style>
            // block was compiled and applied; without it the card is black.
            // Sampled off the rounded corner, which is genuinely transparent.
            expect(pixelAt(raster.rgba, raster.width, raster.width >> 1, 3), g.file).toEqual([
                0x0b, 0x0a, 0x0d,
            ]);

            // Something was actually drawn. A black rectangle scores ~0 here.
            expect(litFraction(raster.rgba), g.file).toBeGreaterThan(0.05);
        }
    });

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

    it('every committed image is up to date with its component and stills', () => {
        for (const g of loaded) {
            const catalog = g.component.catalog as string;
            const committed = sidecarFor(catalog);
            const svg = svgFor(g);
            const raster = rasterize(svg);

            expect({ width: raster.width, height: raster.height }, g.file).toEqual({
                width: committed.width,
                height: committed.height,
            });
            expect(
                normalizedHash(svg),
                `${catalog}.jpg is stale; re-render it:\n` +
                    '  cargo run -q -p darkly --bin export-docs -- --out target/docs/metadata.json\n' +
                    '  node frontend/scripts/render-doc-graphics.mjs --metadata target/docs/metadata.json',
            ).toBe(committed.svg);
        }
    });
});
