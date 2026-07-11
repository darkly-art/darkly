import { describe, it, expect } from 'vitest';
import {
    decodeChunkRecords,
    decoderConfigsCompatible,
    encodeChunkRecord,
    scratchSegmentBinPath,
    segmentBinName,
    segmentDecoderConfig,
    segmentJsonName,
    segmentNumberFromName,
    base64Encode,
    type FramedChunk,
    type SegmentMeta,
} from '../segments';

function chunk(overrides: Partial<FramedChunk> = {}): FramedChunk {
    return {
        key: false,
        timestampUs: 1_720_000_000_000_000,
        data: new Uint8Array([1, 2, 3, 4, 5]),
        ...overrides,
    };
}

function concat(parts: Uint8Array[]): Uint8Array {
    const total = parts.reduce((s, p) => s + p.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const p of parts) {
        out.set(p, off);
        off += p.length;
    }
    return out;
}

describe('chunk framing', () => {
    it('round-trips a sequence of records', () => {
        const chunks = [
            chunk({ key: true, timestampUs: 100, data: new Uint8Array([9]) }),
            chunk({ key: false, timestampUs: 200, data: new Uint8Array(0) }),
            chunk({ key: false, timestampUs: 300, data: new Uint8Array(1000).fill(7) }),
        ];
        const bin = concat(chunks.map(encodeChunkRecord));
        const decoded = decodeChunkRecords(bin);
        expect(decoded).toHaveLength(3);
        decoded.forEach((d, i) => {
            expect(d.key).toBe(chunks[i].key);
            expect(d.timestampUs).toBe(chunks[i].timestampUs);
            expect(Array.from(d.data)).toEqual(Array.from(chunks[i].data));
        });
    });

    it('tolerates non-monotonic timestamps', () => {
        // Wall-clock stamps can go backwards (clock adjust, suspend).
        const chunks = [chunk({ timestampUs: 5000 }), chunk({ timestampUs: 1000 })];
        const decoded = decodeChunkRecords(concat(chunks.map(encodeChunkRecord)));
        expect(decoded.map((d) => d.timestampUs)).toEqual([5000, 1000]);
    });

    it('drops a torn final record at every truncation point', () => {
        const complete = encodeChunkRecord(chunk({ key: true }));
        const torn = encodeChunkRecord(chunk({ data: new Uint8Array(64).fill(3) }));
        const full = concat([complete, torn]);
        // Every strict prefix that cuts into the second record must decode
        // to exactly the first — crash-safe at every byte.
        for (let cut = complete.length; cut < full.length; cut++) {
            const decoded = decodeChunkRecords(full.subarray(0, cut));
            expect(decoded).toHaveLength(1);
            expect(decoded[0].key).toBe(true);
        }
        expect(decodeChunkRecords(full)).toHaveLength(2);
    });

    it('decodes an empty buffer to no records', () => {
        expect(decodeChunkRecords(new Uint8Array(0))).toEqual([]);
    });
});

describe('segment metadata', () => {
    const base: SegmentMeta = {
        n: 1,
        codec: 'avc1.640028',
        width: 1920,
        height: 1080,
        frameCount: 42,
        description: base64Encode(new Uint8Array([1, 100, 0, 40])),
    };

    it('configs with identical codec/dims/description are compatible', () => {
        expect(decoderConfigsCompatible(base, { ...base, n: 2, frameCount: 7 })).toBe(true);
    });

    it('any codec, dimension, or description difference is incompatible', () => {
        expect(decoderConfigsCompatible(base, { ...base, codec: 'vp09.00.10.08' })).toBe(false);
        expect(decoderConfigsCompatible(base, { ...base, width: 1280 })).toBe(false);
        expect(decoderConfigsCompatible(base, { ...base, height: 720 })).toBe(false);
        expect(
            decoderConfigsCompatible(base, {
                ...base,
                description: base64Encode(new Uint8Array([9, 9])),
            }),
        ).toBe(false);
    });

    it('treats absent descriptions as equal (in-band codecs like VP9)', () => {
        const a = { ...base, description: undefined };
        const b = { ...base, description: undefined };
        expect(decoderConfigsCompatible(a, b)).toBe(true);
        expect(decoderConfigsCompatible(a, base)).toBe(false);
    });

    it('reconstructs a decoder config with decoded description bytes', () => {
        const cfg = segmentDecoderConfig(base);
        expect(cfg.codec).toBe('avc1.640028');
        expect(cfg.codedWidth).toBe(1920);
        expect(cfg.codedHeight).toBe(1080);
        expect(Array.from(cfg.description as Uint8Array)).toEqual([1, 100, 0, 40]);
        expect(segmentDecoderConfig({ ...base, description: undefined }).description)
            .toBeUndefined();
    });
});

describe('paths', () => {
    it('scratch and zip names agree on the segment naming scheme', () => {
        expect(segmentBinName(3)).toBe('segment-3.bin');
        expect(segmentJsonName(3)).toBe('segment-3.json');
        expect(scratchSegmentBinPath('tab-a', 2)).toBe('recording-scratch/tab-a/segment-2.bin');
    });

    it('parses segment numbers back out of filenames', () => {
        expect(segmentNumberFromName('segment-12.bin')).toBe(12);
        expect(segmentNumberFromName('segment-1.json')).toBe(1);
        expect(segmentNumberFromName('recording.json')).toBeNull();
        expect(segmentNumberFromName('segment-x.bin')).toBeNull();
        expect(segmentNumberFromName('segment-2.mp4')).toBeNull();
    });
});
