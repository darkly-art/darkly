/**
 * Process-recording encoder worker. Owns the WebCodecs `VideoEncoder` and
 * the OPFS sync-access handles for the current segment — sync handles are
 * a worker-only API and give true appends (the main-thread `createWritable`
 * is atomic-rewrite-on-close, useless for an ever-growing chunk log).
 *
 * Messages in:  `init` (open segment, configure encoder) · `frame`
 * (transferred RGBA buffer → encode) · `flush` (drain encoder, persist
 * segment meta) · `close` (finalize + release everything).
 * Messages out: `ready` · `flushed` · `closed` · `disabled`.
 *
 * Failure policy — encoder errors and storage write errors (including
 * `QuotaExceededError`) share one path: finalize the current segment,
 * retry once on a fresh segment + encoder; a second failure posts
 * `disabled` and capture stops for the session.
 */

import {
    base64Encode,
    encodeChunkRecord,
    segmentBinName,
    segmentJsonName,
    SCRATCH_ROOT,
    type SegmentMeta,
} from './segments';

/** Emit a keyframe at least this often so export can trim/seek cheaply and
 *  a torn tail costs at most this many frames of a segment. */
const KEYFRAME_INTERVAL = 150;

export interface InitMsg {
    type: 'init';
    /** Scratch dir name (`<sessionId>~<recoveryId>` — see `segments.ts`). */
    scratchKey: string;
    segmentN: number;
    codec: string;
    width: number;
    height: number;
    /** Document canvas dims the encoder dims were negotiated against —
     *  persisted per segment for export's aspect-ratio grouping. */
    canvasWidth: number;
    canvasHeight: number;
    bitrate: number;
    fps: number;
}
export interface FrameMsg {
    type: 'frame';
    /** Transferred tightly-packed RGBA, `width × height × 4`. */
    data: ArrayBuffer;
    frameIndex: number;
    /** Wall-clock capture time (µs) — stored in the chunk framing. */
    timestampUs: number;
}
export type WorkerInMsg = InitMsg | FrameMsg | { type: 'flush' } | { type: 'close' };

export type WorkerOutMsg =
    | { type: 'ready'; segmentN: number }
    | { type: 'flushed' }
    | { type: 'closed' }
    | { type: 'disabled'; reason: string };

// OPFS sync-access handles live in `lib.webworker`, which this project's
// dom-lib compilation can't include alongside `lib.dom`; declare the
// minimal surface used here.
interface SyncAccessHandle {
    write(buffer: Uint8Array, options?: { at?: number }): number;
    truncate(size: number): void;
    getSize(): number;
    flush(): void;
    close(): void;
}
type SyncHandleFile = { createSyncAccessHandle(): Promise<SyncAccessHandle> };

const ctx = self as unknown as {
    postMessage(msg: WorkerOutMsg): void;
    onmessage: ((e: MessageEvent<WorkerInMsg>) => void) | null;
    close(): void;
};

/** Everything owned by the currently-open segment. */
interface Segment {
    n: number;
    bin: SyncAccessHandle;
    cursor: number;
    frameCount: number;
    framesSinceKey: number;
    /** From the first chunk's `metadata.decoderConfig`. */
    description: Uint8Array | null;
    encoder: VideoEncoder;
    /** Wall-clock stamps queued at encode time, consumed in chunk-output
     *  order (the encoder emits chunks in submission order). */
    wallClockQueue: number[];
}

let cfg: InitMsg | null = null;
let dir: FileSystemDirectoryHandle | null = null;
let segment: Segment | null = null;
let retried = false;
let disabled = false;

ctx.onmessage = (e) => {
    void dispatch(e.data);
};

async function dispatch(msg: WorkerInMsg): Promise<void> {
    if (disabled && msg.type !== 'close') return;
    try {
        switch (msg.type) {
            case 'init': {
                cfg = msg;
                dir = await scratchDirHandle(msg.scratchKey);
                segment = await openSegment(msg.segmentN);
                ctx.postMessage({ type: 'ready', segmentN: msg.segmentN });
                break;
            }
            case 'frame': {
                if (!segment || !cfg) return;
                const wantKey =
                    segment.frameCount === 0 || segment.framesSinceKey >= KEYFRAME_INTERVAL;
                const frame = new VideoFrame(msg.data, {
                    format: 'RGBA',
                    codedWidth: cfg.width,
                    codedHeight: cfg.height,
                    // Synthetic timeline: frame N plays at N/fps. The wall
                    // clock rides in the chunk framing, not here.
                    timestamp: Math.round((msg.frameIndex * 1e6) / cfg.fps),
                });
                segment.wallClockQueue.push(msg.timestampUs);
                segment.framesSinceKey = wantKey ? 0 : segment.framesSinceKey + 1;
                try {
                    segment.encoder.encode(frame, { keyFrame: wantKey });
                } finally {
                    frame.close();
                }
                break;
            }
            case 'flush': {
                if (segment) {
                    await segment.encoder.flush();
                    writeSegmentMeta(segment);
                }
                ctx.postMessage({ type: 'flushed' });
                break;
            }
            case 'close': {
                await teardown();
                ctx.postMessage({ type: 'closed' });
                ctx.close();
                break;
            }
        }
    } catch (err) {
        await handleFailure(err);
    }
}

async function scratchDirHandle(key: string): Promise<FileSystemDirectoryHandle> {
    const root = await navigator.storage.getDirectory();
    const scratch = await root.getDirectoryHandle(SCRATCH_ROOT, { create: true });
    return scratch.getDirectoryHandle(key, { create: true });
}

async function openSegment(n: number): Promise<Segment> {
    if (!cfg || !dir) throw new Error('worker not initialized');
    const binFile = await dir.getFileHandle(segmentBinName(n), { create: true });
    const bin = await (binFile as unknown as SyncHandleFile).createSyncAccessHandle();
    bin.truncate(0);

    const seg: Segment = {
        n,
        bin,
        cursor: 0,
        frameCount: 0,
        framesSinceKey: 0,
        description: null,
        encoder: null as unknown as VideoEncoder,
        wallClockQueue: [],
    };
    seg.encoder = new VideoEncoder({
        output: (chunk, metadata) => onChunk(seg, chunk, metadata),
        error: (err) => void handleFailure(err),
    });
    seg.encoder.configure({
        codec: cfg.codec,
        width: cfg.width,
        height: cfg.height,
        bitrate: cfg.bitrate,
        framerate: cfg.fps,
        ...(cfg.codec.startsWith('avc1') ? { avc: { format: 'avc' as const } } : {}),
    });
    return seg;
}

function onChunk(
    seg: Segment,
    chunk: EncodedVideoChunk,
    metadata?: EncodedVideoChunkMetadata,
): void {
    try {
        const desc = metadata?.decoderConfig?.description;
        if (desc && !seg.description) {
            seg.description = ArrayBuffer.isView(desc)
                ? new Uint8Array(desc.buffer.slice(desc.byteOffset, desc.byteOffset + desc.byteLength))
                : new Uint8Array((desc as ArrayBuffer).slice(0));
        }
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        const record = encodeChunkRecord({
            key: chunk.type === 'key',
            timestampUs: seg.wallClockQueue.shift() ?? chunk.timestamp,
            data,
        });
        seg.cursor += seg.bin.write(record, { at: seg.cursor });
        seg.bin.flush();
        seg.frameCount++;
    } catch (err) {
        void handleFailure(err);
    }
}

/** Persist `segment-<n>.json`. Runs on flush / close / roll so the meta on
 *  disk always describes the chunks that made it into the `.bin`. */
function writeSegmentMeta(seg: Segment): void {
    if (!cfg || !dir) return;
    const meta: SegmentMeta = {
        n: seg.n,
        codec: cfg.codec,
        width: cfg.width,
        height: cfg.height,
        canvasWidth: cfg.canvasWidth,
        canvasHeight: cfg.canvasHeight,
        frameCount: seg.frameCount,
        ...(seg.description ? { description: base64Encode(seg.description) } : {}),
    };
    void writeSmallFile(segmentJsonName(seg.n), new TextEncoder().encode(JSON.stringify(meta)));
}

async function writeSmallFile(name: string, bytes: Uint8Array): Promise<void> {
    if (!dir) return;
    const fh = await dir.getFileHandle(name, { create: true });
    const handle = await (fh as unknown as SyncHandleFile).createSyncAccessHandle();
    try {
        handle.truncate(0);
        handle.write(bytes, { at: 0 });
        handle.flush();
    } finally {
        handle.close();
    }
}

/** Finalize and release the current segment + encoder. Best-effort: every
 *  step is independently guarded so a broken encoder can't leak the file
 *  handle or vice versa. */
async function teardown(): Promise<void> {
    const seg = segment;
    segment = null;
    if (!seg) return;
    try {
        if (seg.encoder.state === 'configured') await seg.encoder.flush();
    } catch {
        // Encoder already errored — persist what reached the bin.
    }
    try {
        writeSegmentMeta(seg);
    } catch {
        /* best-effort */
    }
    try {
        if (seg.encoder.state !== 'closed') seg.encoder.close();
    } catch {
        /* best-effort */
    }
    try {
        seg.bin.flush();
        seg.bin.close();
    } catch {
        /* best-effort */
    }
}

/** Unified failure path — see the module docs. */
async function handleFailure(err: unknown): Promise<void> {
    if (disabled) return;
    const rolledFrom = segment?.n ?? cfg?.segmentN ?? 0;
    await teardown();
    if (!retried && cfg) {
        retried = true;
        try {
            segment = await openSegment(rolledFrom + 1);
            ctx.postMessage({ type: 'ready', segmentN: rolledFrom + 1 });
            return;
        } catch {
            // Fresh segment failed too — fall through to disable.
        }
    }
    disabled = true;
    ctx.postMessage({
        type: 'disabled',
        reason: err instanceof Error ? err.message : String(err),
    });
}
