/**
 * Encoder negotiation for process recording. Fits the document to the
 * configured long edge, then probes a codec ladder — H.264 first (broad
 * hardware encode support, MP4-native), VP9 as fallback — stepping the
 * resolution down when a config is rejected. Every Darkly-capable browser
 * ships WebCodecs (`VideoEncoder` is a strictly lower bar than WebGPU), so
 * only per-config rejection needs handling, not feature absence.
 *
 * `isConfigSupported` is injectable so the ladder is unit-testable in node.
 */

/** The outcome of a successful negotiation — everything the encoder worker
 *  and the engine's capture target need to agree on. */
export interface NegotiatedCodec {
    codec: string;
    width: number;
    height: number;
    bitrate: number;
    fps: number;
}

export type IsConfigSupported = (
    config: VideoEncoderConfig,
) => Promise<{ supported?: boolean }>;

/** Fixed long-edge fallbacks probed (below the user's cap) when the
 *  preferred resolution is rejected by every codec. */
const FALLBACK_LONG_EDGES = [1920, 1280, 854, 640];

/** Fit `(width, height)` inside `maxLongEdge`, preserving aspect. Both
 *  output dimensions are even (encoder requirement for 4:2:0 content) and
 *  at least 2. Never upscales. */
export function fitToLongEdge(
    width: number,
    height: number,
    maxLongEdge: number,
): { width: number; height: number } {
    const scale = Math.min(1, maxLongEdge / Math.max(width, height));
    const even = (v: number) => Math.max(2, Math.round(v * scale) & ~1);
    return { width: even(width), height: even(height) };
}

/** Bitrate policy: 0.07 bits per pixel per frame, clamped to 1–12 Mbps
 *  (~4 Mbps at 1080p30). Probed as part of the codec config so a rejection
 *  steps the ladder like any other constraint. */
export function bitrateFor(width: number, height: number, fps: number): number {
    return Math.min(12e6, Math.max(1e6, Math.round(width * height * fps * 0.07)));
}

/** H.264 High-profile codec string with a level adequate for the frame
 *  size at 30 fps (4.0 covers 1080p, 5.1 covers 1440p, 5.2 covers 4K). */
export function h264CodecFor(longEdge: number): string {
    if (longEdge > 2560) return 'avc1.640034'; // High 5.2
    if (longEdge > 1920) return 'avc1.640033'; // High 5.1
    return 'avc1.640028'; // High 4.0
}

const VP9_CODEC = 'vp09.00.10.08'; // Profile 0, level 1.0, 8-bit

/**
 * Negotiate an encoder config for a document of `docWidth × docHeight`,
 * capped at `maxLongEdge`. Tries H.264 then VP9 at the preferred
 * resolution, then steps the resolution down and repeats. Returns null
 * only when every rung is rejected (no capture this session).
 */
export async function negotiateCodec(opts: {
    docWidth: number;
    docHeight: number;
    maxLongEdge: number;
    fps: number;
    isConfigSupported?: IsConfigSupported;
}): Promise<NegotiatedCodec | null> {
    const probe: IsConfigSupported =
        opts.isConfigSupported ?? ((c) => VideoEncoder.isConfigSupported(c));

    const preferred = Math.max(2, Math.round(opts.maxLongEdge));
    const ladder = [preferred, ...FALLBACK_LONG_EDGES.filter((e) => e < preferred)];

    for (const longEdge of ladder) {
        const { width, height } = fitToLongEdge(opts.docWidth, opts.docHeight, longEdge);
        const bitrate = bitrateFor(width, height, opts.fps);
        for (const codec of [h264CodecFor(Math.max(width, height)), VP9_CODEC]) {
            const config: VideoEncoderConfig = {
                codec,
                width,
                height,
                bitrate,
                framerate: opts.fps,
                ...(codec.startsWith('avc1') ? { avc: { format: 'avc' as const } } : {}),
            };
            try {
                const result = await probe(config);
                if (result.supported) {
                    return { codec, width, height, bitrate, fps: opts.fps };
                }
            } catch {
                // A throwing probe counts as a rejection — step the ladder.
            }
        }
    }
    return null;
}
