import { describe, it, expect } from 'vitest';
import {
    bitrateFor,
    fitToLongEdge,
    h264CodecFor,
    negotiateCodec,
    type IsConfigSupported,
} from '../codec';

/** Fake probe accepting only configs the predicate approves; records every
 *  probed config so tests can assert the ladder order. */
function fakeProbe(accept: (c: VideoEncoderConfig) => boolean): {
    probe: IsConfigSupported;
    probed: VideoEncoderConfig[];
} {
    const probed: VideoEncoderConfig[] = [];
    return {
        probed,
        probe: async (c) => {
            probed.push(c);
            return { supported: accept(c) };
        },
    };
}

describe('fitToLongEdge', () => {
    it('fits the long edge and preserves aspect, even-aligned', () => {
        expect(fitToLongEdge(3840, 2160, 1920)).toEqual({ width: 1920, height: 1080 });
        expect(fitToLongEdge(2160, 3840, 1920)).toEqual({ width: 1080, height: 1920 });
    });

    it('never upscales', () => {
        expect(fitToLongEdge(800, 600, 1920)).toEqual({ width: 800, height: 600 });
    });

    it('forces even dimensions', () => {
        const { width, height } = fitToLongEdge(1001, 333, 1920);
        expect(width % 2).toBe(0);
        expect(height % 2).toBe(0);
    });

    it('clamps degenerate dimensions up to 2', () => {
        expect(fitToLongEdge(10000, 1, 1920).height).toBe(2);
    });
});

describe('bitrateFor', () => {
    it('is ~4 Mbps at 1080p30', () => {
        const b = bitrateFor(1920, 1080, 30);
        expect(b).toBeGreaterThan(4e6);
        expect(b).toBeLessThan(5e6);
    });

    it('clamps to the 1–12 Mbps envelope', () => {
        expect(bitrateFor(64, 64, 30)).toBe(1e6);
        expect(bitrateFor(3840, 2160, 30)).toBe(12e6);
    });
});

describe('h264CodecFor', () => {
    it('raises the level with the frame size', () => {
        expect(h264CodecFor(1920)).toBe('avc1.640028');
        expect(h264CodecFor(2560)).toBe('avc1.640033');
        expect(h264CodecFor(3840)).toBe('avc1.640034');
    });
});

describe('negotiateCodec', () => {
    it('prefers H.264 at the fitted resolution', async () => {
        const { probe } = fakeProbe(() => true);
        const result = await negotiateCodec({
            docWidth: 3840,
            docHeight: 2160,
            maxLongEdge: 1920,
            fps: 30,
            isConfigSupported: probe,
        });
        expect(result).toEqual({
            codec: 'avc1.640028',
            width: 1920,
            height: 1080,
            bitrate: bitrateFor(1920, 1080, 30),
            fps: 30,
        });
    });

    it('falls back to VP9 at the same resolution before stepping down', async () => {
        const { probe, probed } = fakeProbe((c) => c.codec.startsWith('vp09'));
        const result = await negotiateCodec({
            docWidth: 1920,
            docHeight: 1080,
            maxLongEdge: 1920,
            fps: 30,
            isConfigSupported: probe,
        });
        expect(result?.codec).toBe('vp09.00.10.08');
        expect(result?.width).toBe(1920);
        expect(probed[0].codec.startsWith('avc1')).toBe(true);
    });

    it('steps the resolution down when both codecs reject a rung', async () => {
        const { probe } = fakeProbe((c) => Math.max(c.width, c.height) <= 1280);
        const result = await negotiateCodec({
            docWidth: 3840,
            docHeight: 2160,
            maxLongEdge: 3840,
            fps: 30,
            isConfigSupported: probe,
        });
        expect(result?.width).toBe(1280);
        expect(result?.height).toBe(720);
        expect(result?.codec).toBe('avc1.640028');
    });

    it('returns null when every rung is rejected', async () => {
        const { probe } = fakeProbe(() => false);
        const result = await negotiateCodec({
            docWidth: 1920,
            docHeight: 1080,
            maxLongEdge: 1920,
            fps: 30,
            isConfigSupported: probe,
        });
        expect(result).toBeNull();
    });

    it('treats a throwing probe as a rejection and keeps stepping', async () => {
        let first = true;
        const probe: IsConfigSupported = async (c) => {
            if (first) {
                first = false;
                throw new DOMException('bad config');
            }
            return { supported: true };
        };
        const result = await negotiateCodec({
            docWidth: 1920,
            docHeight: 1080,
            maxLongEdge: 1920,
            fps: 30,
            isConfigSupported: probe,
        });
        expect(result).not.toBeNull();
    });

    it('probes the bitrate policy as part of the config', async () => {
        const { probe, probed } = fakeProbe(() => true);
        await negotiateCodec({
            docWidth: 1920,
            docHeight: 1080,
            maxLongEdge: 1920,
            fps: 30,
            isConfigSupported: probe,
        });
        expect(probed[0].bitrate).toBe(bitrateFor(1920, 1080, 30));
    });
});
