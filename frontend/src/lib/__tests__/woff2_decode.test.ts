import { describe, it, expect } from 'vitest';
// @ts-ignore — Node builtin; the project intentionally omits @types/node (see
// vite.config.ts). Vitest runs under node, so this resolves at runtime.
import { readFileSync } from 'node:fs';
// The REAL decoder — deliberately not mocked. `google_fonts.test.ts` mocks this
// module to test the pipeline wiring; this file exercises the actual WASM decode
// so a broken/renamed/hanging decoder can't pass silently (the fully-mocked
// pipeline test would). NB: this runs in node — it guards the decoder's contract
// (import path, export shape, resolves without hanging, valid SFNT out), but a
// browser-only bundler failure still needs a real-browser check.
import decompress from 'woff2-encoder/decompress';

// Cantarell-VF compressed to woff2 (see fixtures/NOTICE.md). OFL 1.1. Path is
// relative to the vitest cwd (the frontend project root).
const woff2 = new Uint8Array(
    readFileSync('src/lib/__tests__/fixtures/Cantarell-VF.woff2'),
);

describe('woff2-encoder/decompress (real decoder)', () => {
    it('decompresses a woff2 to valid SFNT without hanging', async () => {
        const sfnt = await decompress(woff2);
        expect(sfnt.length).toBeGreaterThan(woff2.length);
        // SFNT magic: "OTTO" (OpenType/CFF) or 0x00010000 (TrueType).
        const m = Array.from(sfnt.slice(0, 4));
        const isOtto = String.fromCharCode(...m) === 'OTTO';
        const isTrueType = m[0] === 0 && m[1] === 1 && m[2] === 0 && m[3] === 0;
        expect(isOtto || isTrueType).toBe(true);
    });
});
