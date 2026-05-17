import { describe, it, expect } from 'vitest';
import { detectKind, isImageKind } from '../detectKind';

/** Build a Uint8Array from a magic-byte prefix + arbitrary tail. The
 *  detector only sniffs the first few bytes — anything past the
 *  signature is irrelevant content padding. */
function withPrefix(prefix: number[], tailLen = 0): Uint8Array {
    const out = new Uint8Array(prefix.length + tailLen);
    out.set(prefix);
    return out;
}

describe('detectKind', () => {
    it('identifies a .darkly zip by `PK\\x03\\x04`', () => {
        const bytes = withPrefix([0x50, 0x4b, 0x03, 0x04], 100);
        expect(detectKind(bytes)).toBe('darkly');
    });

    it('identifies PNG by `\\x89PNG`', () => {
        const bytes = withPrefix([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], 100);
        expect(detectKind(bytes)).toBe('png');
    });

    it('identifies JPEG by `\\xff\\xd8\\xff`', () => {
        const bytes = withPrefix([0xff, 0xd8, 0xff, 0xe0], 100);
        expect(detectKind(bytes)).toBe('jpeg');
    });

    it('identifies WebP by `RIFF...WEBP`', () => {
        const bytes = withPrefix(
            [
                0x52, 0x49, 0x46, 0x46, // RIFF
                0x10, 0x00, 0x00, 0x00, // size (ignored by detector)
                0x57, 0x45, 0x42, 0x50, // WEBP
            ],
            32,
        );
        expect(detectKind(bytes)).toBe('webp');
    });

    it('returns "unknown" for buffers too short to identify', () => {
        expect(detectKind(new Uint8Array([0x50, 0x4b]))).toBe('unknown');
        expect(detectKind(new Uint8Array([]))).toBe('unknown');
    });

    it('returns "unknown" for unrecognized signatures', () => {
        expect(detectKind(new Uint8Array([0x00, 0x01, 0x02, 0x03]))).toBe('unknown');
        // GIF: not supported today (not in the Open picker filter).
        expect(detectKind(new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]))).toBe('unknown');
    });

    it('does NOT false-positive WebP on a bare RIFF without WEBP tag', () => {
        // RIFF + matching size + non-WEBP fourcc (e.g. WAVE) — not an image.
        const bytes = withPrefix(
            [
                0x52, 0x49, 0x46, 0x46,
                0x10, 0x00, 0x00, 0x00,
                0x57, 0x41, 0x56, 0x45, // WAVE, not WEBP
            ],
            16,
        );
        expect(detectKind(bytes)).toBe('unknown');
    });
});

describe('isImageKind', () => {
    it('matches the three image formats and nothing else', () => {
        expect(isImageKind('png')).toBe(true);
        expect(isImageKind('jpeg')).toBe(true);
        expect(isImageKind('webp')).toBe(true);
        expect(isImageKind('darkly')).toBe(false);
        expect(isImageKind('unknown')).toBe(false);
    });
});
