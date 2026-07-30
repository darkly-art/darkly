/**
 * Timelapse export — turn a tab's recorded segments into an MP4 or GIF at
 * user-chosen options (playback rate, output resolution, target aspect
 * ratio + stretch/fit/fill conversion for segments of other aspects — see
 * `exportOptions.ts`).
 *
 * MP4 (primary): when every segment shares a decoder config AND the
 * requested resolution is exactly the packets' ({@link canPassthrough}),
 * the encoded packets pass straight through into the MP4 container via
 * Mediabunny's `EncodedVideoPacketSource` — a near-free re-mux, no decode,
 * at any playback rate. Anything else (mixed aspects, non-native
 * resolution) decodes and re-encodes at a probed encoder config for the
 * requested dims, drawing each frame per the conversion method.
 *
 * GIF (secondary): decode → draw per the conversion method at the
 * requested dims → gifenc, one GIF frame per recorded frame.
 *
 * All timestamps are synthetic — frame N plays at N/fps — regardless of
 * the (possibly non-monotonic) wall-clock stamps in the chunk framing.
 */
// gifenc: https://github.com/mattdesl/gifenc by Matt DesLauriers (@mattdesl).
import { GIFEncoder, quantize, applyPalette } from 'gifenc';
import {
    BufferTarget,
    EncodedPacket,
    EncodedVideoPacketSource,
    Mp4OutputFormat,
    Output,
    VideoSample,
    VideoSampleSource,
    type VideoCodec,
} from 'mediabunny';
import type { DarklyInstance } from '../state/app.svelte';
import { processRecording } from './recorder.svelte';
import { negotiateCodec } from './codec';
import {
    canPassthrough,
    computeDrawRect,
    gifDelayMs,
    groupSegmentsByAspect,
    type AspectGroup,
    type ConversionMethod,
} from './exportOptions';
import { decodeChunkRecords, segmentDecoderConfig, type SegmentMeta } from './segments';

/** One tab's recording, summarized for the export modal. */
export interface RecordingInfo {
    frameCount: number;
    /** On-disk size of the encoded segments, in bytes. */
    byteSize: number;
    segmentCount: number;
    /** Segments grouped by document aspect ratio — more than one group
     *  means the canvas aspect changed mid-recording and the modal offers
     *  the target-aspect + conversion choice. */
    groups: AspectGroup[];
    /** The last segment's group: the document's final shape. */
    defaultGroupIndex: number;
}

/** User-chosen export parameters, sanitized by the modal
 *  (`clampFps` / `lockedDims`). */
export interface TimelapseExportOptions {
    fps: number;
    width: number;
    height: number;
    /** How segments whose aspect differs from the output's are drawn. */
    method: ConversionMethod;
}

type Segment = { meta: SegmentMeta; bin: Uint8Array };

/** Summarize a tab's recording, or null when it has none. */
export async function readRecordingInfo(inst: DarklyInstance): Promise<RecordingInfo | null> {
    const segments = await processRecording.readRecording(inst);
    if (segments.length === 0) return null;
    const { groups, defaultIndex } = groupSegmentsByAspect(segments.map((s) => s.meta));
    return {
        frameCount: segments.reduce((s, seg) => s + seg.meta.frameCount, 0),
        byteSize: segments.reduce((s, seg) => s + seg.bin.length, 0),
        segmentCount: segments.length,
        groups,
        defaultGroupIndex: defaultIndex,
    };
}

function mediabunnyCodec(codec: string): VideoCodec {
    return codec.startsWith('avc1') ? 'avc' : 'vp9';
}

/** Export the tab's recording as an MP4 blob. */
export async function exportTimelapseMp4(
    inst: DarklyInstance,
    opts: TimelapseExportOptions,
): Promise<Blob> {
    const segments = await processRecording.readRecording(inst);
    if (segments.length === 0) throw new Error('no recording to export');

    const target = new BufferTarget();
    const output = new Output({ format: new Mp4OutputFormat(), target });

    if (canPassthrough(segments.map((s) => s.meta), opts.width, opts.height)) {
        await muxPassthrough(output, segments, opts.fps);
    } else {
        await transcode(output, segments, opts);
    }
    if (!target.buffer) throw new Error('mux produced no output');
    return new Blob([target.buffer], { type: 'video/mp4' });
}

/** Packet passthrough: re-stamp every chunk with continuous synthetic
 *  timestamps across segments (each segment leads with a keyframe by
 *  construction) and hand the encoded bytes straight to the muxer. */
async function muxPassthrough(output: Output, segments: Segment[], fps: number): Promise<void> {
    const source = new EncodedVideoPacketSource(mediabunnyCodec(segments[0].meta.codec));
    output.addVideoTrack(source, { frameRate: fps });
    await output.start();

    let frameN = 0;
    let first = true;
    for (const seg of segments) {
        for (const chunk of decodeChunkRecords(seg.bin)) {
            const packet = new EncodedPacket(
                chunk.data,
                chunk.key ? 'key' : 'delta',
                frameN / fps,
                1 / fps,
            );
            await source.add(
                packet,
                first ? { decoderConfig: segmentDecoderConfig(seg.meta) } : undefined,
            );
            first = false;
            frameN++;
        }
    }
    await output.finalize();
}

/** Decode → uniform re-encode at the requested resolution, drawing each
 *  frame per the conversion method. The encoder config is probed for the
 *  requested dims (`negotiateCodec` steps the resolution ladder down on
 *  rejection, so unsupported sizes shrink rather than fail). */
async function transcode(
    output: Output,
    segments: Segment[],
    opts: TimelapseExportOptions,
): Promise<void> {
    const negotiated = await negotiateCodec({
        docWidth: opts.width,
        docHeight: opts.height,
        maxLongEdge: Math.max(opts.width, opts.height),
        fps: opts.fps,
    });
    if (!negotiated) throw new Error('no supported encoder config for the requested resolution');
    const { width, height } = negotiated;
    const source = new VideoSampleSource({
        codec: mediabunnyCodec(negotiated.codec),
        bitrate: negotiated.bitrate,
    });
    output.addVideoTrack(source, { frameRate: opts.fps });
    await output.start();

    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('2d context unavailable');

    let frameN = 0;
    for (const seg of segments) {
        await decodeSegmentFrames(seg, async (frame) => {
            ctx.fillStyle = '#000';
            ctx.fillRect(0, 0, width, height);
            const r = computeDrawRect(
                opts.method,
                frame.displayWidth,
                frame.displayHeight,
                width,
                height,
            );
            ctx.drawImage(frame, r.x, r.y, r.w, r.h);
            frame.close();
            const sample = new VideoSample(canvas, {
                timestamp: frameN / opts.fps,
                duration: 1 / opts.fps,
            });
            frameN++;
            await source.add(sample);
            sample.close();
        });
    }
    await output.finalize();
}

/** Export the tab's recording as a looping GIF blob. */
export async function exportTimelapseGif(
    inst: DarklyInstance,
    opts: TimelapseExportOptions,
): Promise<Blob> {
    const segments = await processRecording.readRecording(inst);
    if (segments.length === 0) throw new Error('no recording to export');

    const { width, height } = opts;
    const delayMs = gifDelayMs(opts.fps);

    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (!ctx) throw new Error('2d context unavailable');

    const gif = GIFEncoder();
    for (const seg of segments) {
        await decodeSegmentFrames(seg, async (frame) => {
            ctx.fillStyle = '#000';
            ctx.fillRect(0, 0, width, height);
            const r = computeDrawRect(
                opts.method,
                frame.displayWidth,
                frame.displayHeight,
                width,
                height,
            );
            ctx.drawImage(frame, r.x, r.y, r.w, r.h);
            frame.close();
            const { data } = ctx.getImageData(0, 0, width, height);
            const palette = quantize(data, 256);
            const indexed = applyPalette(data, palette);
            gif.writeFrame(indexed, width, height, { palette, delay: delayMs });
        });
    }
    gif.finish();
    // BlobPart requires Uint8Array<ArrayBuffer>; gifenc's buffer is typed
    // ArrayBufferLike. The bytes are plain (non-shared) — cast is safe.
    return new Blob([gif.bytes() as Uint8Array<ArrayBuffer>], { type: 'image/gif' });
}

/**
 * Decode one segment's chunks, invoking `onFrame` for every decoded frame
 * **sequentially** (the next decode output waits for the previous
 * handler). The handler owns the frame and must close it.
 */
async function decodeSegmentFrames(
    seg: Segment,
    onFrame: (frame: VideoFrame) => Promise<void>,
): Promise<void> {
    let chain: Promise<void> = Promise.resolve();
    let failure: unknown = null;
    const decoder = new VideoDecoder({
        output: (frame) => {
            chain = chain.then(() => onFrame(frame)).catch((e) => {
                failure = failure ?? e;
                frame.close();
            });
        },
        error: (e) => {
            failure = failure ?? e;
        },
    });
    decoder.configure(segmentDecoderConfig(seg.meta));
    for (const chunk of decodeChunkRecords(seg.bin)) {
        decoder.decode(
            new EncodedVideoChunk({
                type: chunk.key ? 'key' : 'delta',
                timestamp: chunk.timestampUs,
                data: chunk.data,
            }),
        );
    }
    await decoder.flush();
    await chain;
    decoder.close();
    if (failure) throw failure instanceof Error ? failure : new Error(String(failure));
}
