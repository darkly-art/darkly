// wawoff2 is a WASM (Emscripten) module shipping no type declarations. It
// exposes two async functions that convert between woff2 and raw SFNT bytes.
// We only use `decompress` (woff2 → TTF/OTF) on the Google-import path.
declare module 'wawoff2' {
    export function decompress(input: Uint8Array): Promise<Uint8Array>;
    export function compress(input: Uint8Array): Promise<Uint8Array>;
}
