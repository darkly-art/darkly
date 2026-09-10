import { describe, it, expect, vi, beforeEach } from 'vitest';

// The import pipeline hands decoded bytes to the personal library: spy on it.
const { addSpy } = vi.hoisted(() => ({
    addSpy: vi.fn((_bytes: Uint8Array, _source: string) => Promise.resolve(['Roboto'])),
}));
vi.mock('../../state/font_library.svelte', () => ({
    fontLibrary: { add: addSpy },
}));

// woff2-encoder/decompress is a WASM decoder (default export); stand in with an
// identity "decode".
const { decompressSpy } = vi.hoisted(() => ({
    decompressSpy: vi.fn((buf: Uint8Array) => Promise.resolve(new Uint8Array([0x00, 0x01, ...buf]))),
}));
vi.mock('woff2-encoder/decompress', () => ({ default: decompressSpy }));

import { cssUrl, previewUrl, importFont, type CatalogFont } from '../google_fonts';

describe('cssUrl', () => {
    it('requests the full weight range as a single variable file', () => {
        expect(cssUrl('Roboto', [{ tag: 'wght', min: 100, max: 900 }])).toBe(
            'https://fonts.googleapis.com/css2?family=Roboto:wght@100..900&display=swap',
        );
    });

    it('plus-encodes multi-word families and omits the axis when static', () => {
        expect(cssUrl('Open Sans', [])).toBe(
            'https://fonts.googleapis.com/css2?family=Open+Sans&display=swap',
        );
    });

    it('requests the italic face across the weight range', () => {
        expect(
            cssUrl('Inter', [{ tag: 'wght', min: 100, max: 900 }], { italic: true }),
        ).toBe(
            'https://fonts.googleapis.com/css2?family=Inter:ital,wght@1,100..900&display=swap',
        );
    });

    it('requests a static italic face when the family has no weight axis', () => {
        expect(cssUrl('Lobster', [], { italic: true })).toBe(
            'https://fonts.googleapis.com/css2?family=Lobster:ital@1&display=swap',
        );
    });
});

describe('previewUrl', () => {
    const roboto: CatalogFont = {
        family: 'Roboto',
        category: 'Sans Serif',
        axes: [{ tag: 'wght', min: 100, max: 900 }],
        italic: true,
        subsets: ['latin'],
        popularity: 1,
    };

    it('uses the standard css2 embed', () => {
        expect(previewUrl(roboto)).toBe(
            'https://fonts.googleapis.com/css2?family=Roboto:wght@100..900&display=swap',
        );
    });

    // Regression: never subset previews via `&text=`. That endpoint (`/l/font`)
    // is CORS-flaky for cross-origin @font-face loads and blocks the preview;
    // the plain `/s/` woff2 embed is the reliable path.
    it('never requests the CORS-flaky text-subset endpoint', () => {
        expect(previewUrl(roboto)).not.toContain('text=');
    });
});

describe('importFont', () => {
    const cssText =
        "/* latin */\n@font-face { font-family: 'Roboto';\n" +
        '  src: url(https://fonts.gstatic.com/s/roboto/x.woff2) format("woff2");\n' +
        '  unicode-range: U+0000-00FF, U+0131; }';
    const woff2Bytes = new Uint8Array([0x77, 0x4f, 0x46, 0x32]); // "wOF2"

    function stubFetch() {
        const fetchMock = vi.fn((url: string) => {
            if (url.includes('css2')) {
                return Promise.resolve({ text: () => Promise.resolve(cssText) });
            }
            return Promise.resolve({ arrayBuffer: () => Promise.resolve(woff2Bytes.buffer) });
        });
        vi.stubGlobal('fetch', fetchMock);
        return fetchMock;
    }

    beforeEach(() => {
        addSpy.mockClear();
        decompressSpy.mockClear();
    });

    it('pipes css2 → latin woff2 → decode → library.add', async () => {
        const fetchMock = stubFetch();

        const font: CatalogFont = {
            family: 'Roboto',
            category: 'Sans Serif',
            axes: [{ tag: 'wght', min: 100, max: 900 }],
            italic: false,
            subsets: ['latin'],
            popularity: 1,
        };
        const families = await importFont(font);

        // Fetched the css2 URL first, then the gstatic woff2.
        expect(fetchMock.mock.calls[0][0]).toContain('css2?family=Roboto:wght@100..900');
        expect(fetchMock.mock.calls[1][0]).toBe('https://fonts.gstatic.com/s/roboto/x.woff2');
        // Decoded the woff2, then handed the TTF to the library as a Google font.
        expect(decompressSpy).toHaveBeenCalledOnce();
        expect(addSpy).toHaveBeenCalledOnce();
        expect(addSpy.mock.calls[0][1]).toBe('google');
        const passed = addSpy.mock.calls[0][0] as Uint8Array;
        expect(Array.from(passed.slice(0, 2))).toEqual([0x00, 0x01]); // decode output
        expect(families).toEqual(['Roboto']);

        vi.unstubAllGlobals();
    });

    // Regression: the Italic control was inert because import only registered the
    // upright face. Italic-capable families must also fetch and register an
    // italic face so parley's FontStyle::Italic has a real face to match.
    it('registers a second italic face when the family ships italic', async () => {
        const fetchMock = stubFetch();

        const font: CatalogFont = {
            family: 'Roboto',
            category: 'Sans Serif',
            axes: [{ tag: 'wght', min: 100, max: 900 }],
            italic: true,
            subsets: ['latin'],
            popularity: 1,
        };
        const families = await importFont(font);

        // Two css2 fetches: upright, then the italic face.
        const cssCalls = fetchMock.mock.calls
            .map((c) => c[0] as string)
            .filter((u) => u.includes('css2'));
        expect(cssCalls).toHaveLength(2);
        expect(cssCalls[0]).toContain('family=Roboto:wght@100..900');
        expect(cssCalls[1]).toContain('family=Roboto:ital,wght@1,100..900');
        // Both faces registered under the family.
        expect(addSpy).toHaveBeenCalledTimes(2);
        // The reported families come from the upright import.
        expect(families).toEqual(['Roboto']);

        vi.unstubAllGlobals();
    });

    it('registers only the upright face when the family has no italic', async () => {
        stubFetch();

        const font: CatalogFont = {
            family: 'Roboto',
            category: 'Sans Serif',
            axes: [{ tag: 'wght', min: 100, max: 900 }],
            italic: false,
            subsets: ['latin'],
            popularity: 1,
        };
        await importFont(font);

        expect(addSpy).toHaveBeenCalledOnce();

        vi.unstubAllGlobals();
    });
});
