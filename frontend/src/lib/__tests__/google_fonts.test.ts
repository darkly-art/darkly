import { describe, it, expect, vi, beforeEach } from 'vitest';

// The import pipeline hands decoded bytes to the personal library — spy on it.
const { addSpy } = vi.hoisted(() => ({
    addSpy: vi.fn((_bytes: Uint8Array, _source: string) => Promise.resolve(['Roboto'])),
}));
vi.mock('../../state/font_library.svelte', () => ({
    fontLibrary: { add: addSpy },
}));

// wawoff2 is a WASM decoder; stand in with an identity "decode".
const { decompressSpy } = vi.hoisted(() => ({
    decompressSpy: vi.fn((buf: Uint8Array) => Promise.resolve(new Uint8Array([0x00, 0x01, ...buf]))),
}));
vi.mock('wawoff2', () => ({ decompress: decompressSpy }));

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

    it('adds the ital axis alongside wght when italic is requested', () => {
        expect(
            cssUrl(
                'Inter',
                [
                    { tag: 'wght', min: 100, max: 900 },
                    { tag: 'ital', min: 0, max: 1 },
                ],
                { italic: true },
            ),
        ).toBe(
            'https://fonts.googleapis.com/css2?family=Inter:ital,wght@0,100..900;1,100..900&display=swap',
        );
    });

});

describe('previewUrl', () => {
    const roboto: CatalogFont = {
        family: 'Roboto',
        category: 'Sans Serif',
        axes: [{ tag: 'wght', min: 100, max: 900 }],
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
    beforeEach(() => {
        addSpy.mockClear();
        decompressSpy.mockClear();
    });

    it('pipes css2 → latin woff2 → decode → library.add', async () => {
        const cssText =
            "/* latin */\n@font-face { font-family: 'Roboto';\n" +
            '  src: url(https://fonts.gstatic.com/s/roboto/x.woff2) format("woff2");\n' +
            '  unicode-range: U+0000-00FF, U+0131; }';
        const woff2Bytes = new Uint8Array([0x77, 0x4f, 0x46, 0x32]); // "wOF2"

        const fetchMock = vi.fn((url: string) => {
            if (url.includes('css2')) {
                return Promise.resolve({ text: () => Promise.resolve(cssText) });
            }
            return Promise.resolve({ arrayBuffer: () => Promise.resolve(woff2Bytes.buffer) });
        });
        vi.stubGlobal('fetch', fetchMock);

        const font: CatalogFont = {
            family: 'Roboto',
            category: 'Sans Serif',
            axes: [{ tag: 'wght', min: 100, max: 900 }],
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
});
