/**
 * Process-recording storage model: pure helpers shared by the recorder
 * service, the encoder worker, the save/open integration, and the export
 * pipeline. No DOM, no OPFS: everything here is unit-testable in node.
 *
 * ## Layout
 *
 * The **working recording** lives in OPFS scratch, keyed by
 * `<sessionId>~<recoveryId>`, the same attribution the crash-recovery
 * store encodes in its snapshot filenames, so scratch dirs share the
 * snapshots' exact lifecycle (offered for adoption after a crash,
 * garbage-collected as orphans of cleanly-exited sessions at boot):
 *
 *     recording-scratch/<key>/recording.json    stream params
 *     recording-scratch/<key>/segment-<n>.json  per-segment meta
 *     recording-scratch/<key>/segment-<n>.bin   framed encoded chunks
 *
 * On save the same files are embedded in the `.darkly` zip under
 * `recording/` (Procreate's `video/segments/` shape); on open they are
 * extracted back into the new tab's scratch. One segment per encoder run
 * (app session, resolution change, or error-recovery roll); each leads
 * with a keyframe by construction, so export can concatenate them.
 *
 * ## Chunk framing (`segment-<n>.bin`)
 *
 * A flat sequence of records, each:
 *
 *     [u32le payload length][u8 keyframe flag][u64le timestampUs][payload]
 *
 * `timestampUs` is the **wall-clock** capture time in microseconds. It may
 * be non-monotonic (system clock adjustment, suspend/resume), so consumers
 * must tolerate that; export re-stamps frames with synthetic timestamps
 * (frame N plays at N/fps) and ignores this field.
 *
 * The format is crash-safe at every byte: a torn final record (interrupted
 * write) is detected by length and dropped by the decoder.
 */

/** Fixed playback rate for exported timelapses. Captures are irregular
 *  (change-triggered); every frame simply plays for 1/fps seconds. */
export const RECORDING_FPS = 30;

/** Bump when the scratch/zip layout or framing changes. Pre-release: a
 *  version mismatch means "discard the recording", never "migrate". */
export const RECORDING_VERSION = 2;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/** OPFS root directory for all per-tab recording scratch. */
export const SCRATCH_ROOT = 'recording-scratch';
/** Directory inside the `.darkly` zip holding the embedded recording. */
export const ZIP_DIR = 'recording';

const MANIFEST_NAME = 'recording.json';

/** Separator between session id and recovery id in a scratch dir name,
 *  the same convention as the recovery store's snapshot filenames; neither
 *  id contains `~`, so parsing back is unambiguous. */
const KEY_SEP = '~';

/** The scratch dir name for a tab: session + tab attribution in one key. */
export function scratchKey(sessionId: string, recoveryId: string): string {
    return `${sessionId}${KEY_SEP}${recoveryId}`;
}

/** Parse the owning session id back out of a scratch dir name, or null
 *  for a malformed name (removed as garbage by the boot sweep). */
export function sessionIdOfScratchKey(key: string): string | null {
    const sep = key.indexOf(KEY_SEP);
    return sep > 0 && sep < key.length - 1 ? key.slice(0, sep) : null;
}

export function scratchDir(key: string): string {
    return `${SCRATCH_ROOT}/${key}`;
}

export function scratchManifestPath(key: string): string {
    return `${scratchDir(key)}/${MANIFEST_NAME}`;
}

export function scratchSegmentJsonPath(key: string, n: number): string {
    return `${scratchDir(key)}/${segmentJsonName(n)}`;
}

export function scratchSegmentBinPath(key: string, n: number): string {
    return `${scratchDir(key)}/${segmentBinName(n)}`;
}

export function zipManifestPath(): string {
    return `${ZIP_DIR}/${MANIFEST_NAME}`;
}

export function segmentJsonName(n: number): string {
    return `segment-${n}.json`;
}

export function segmentBinName(n: number): string {
    return `segment-${n}.bin`;
}

/** Parse the segment number out of a `segment-<n>.json` / `segment-<n>.bin`
 *  filename, or null for anything else (the manifest, strays). */
export function segmentNumberFromName(name: string): number | null {
    const m = /^segment-(\d+)\.(json|bin)$/.exec(name);
    return m ? parseInt(m[1], 10) : null;
}

// ---------------------------------------------------------------------------
// Metadata types
// ---------------------------------------------------------------------------

/** `recording.json`: stream-global parameters. Everything per-segment
 *  (codec, dims, frame count, decoder description) lives in the segment's
 *  own JSON so there is exactly one home per fact. */
export interface RecordingManifest {
    version: number;
    fps: number;
}

/** `segment-<n>.json`: everything a decoder needs to play the segment's
 *  `.bin`, written when the segment finalizes (flush / close / roll). */
export interface SegmentMeta {
    n: number;
    /** WebCodecs codec string, e.g. `avc1.640028` or `vp09.00.10.08`. */
    codec: string;
    /** Encoder frame dimensions (even-aligned fit of the canvas). */
    width: number;
    height: number;
    /** Document canvas dimensions during this segment, the exact aspect
     *  ratio export groups by (the encoder fit perturbs the ratio at small
     *  sizes). */
    canvasWidth: number;
    canvasHeight: number;
    frameCount: number;
    /** Base64-encoded `VideoDecoderConfig.description` (avcC / vpcC bytes),
     *  absent when the codec carries its config in-band. */
    description?: string;
}

/** True when packets from `b` can be appended to a stream configured from
 *  `a` without re-encoding: same codec, dims, and decoder description. */
export function decoderConfigsCompatible(a: SegmentMeta, b: SegmentMeta): boolean {
    return (
        a.codec === b.codec &&
        a.width === b.width &&
        a.height === b.height &&
        (a.description ?? '') === (b.description ?? '')
    );
}

/** Reconstruct the WebCodecs decoder config for a segment. */
export function segmentDecoderConfig(meta: SegmentMeta): VideoDecoderConfig {
    return {
        codec: meta.codec,
        codedWidth: meta.width,
        codedHeight: meta.height,
        ...(meta.description !== undefined
            ? { description: base64Decode(meta.description) }
            : {}),
    };
}

// ---------------------------------------------------------------------------
// Base64 (binary decoder descriptions inside JSON)
// ---------------------------------------------------------------------------

export function base64Encode(bytes: Uint8Array): string {
    let binary = '';
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return btoa(binary);
}

export function base64Decode(text: string): Uint8Array {
    const binary = atob(text);
    const out = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
    return out;
}

// ---------------------------------------------------------------------------
// Chunk framing
// ---------------------------------------------------------------------------

/** One encoded video chunk as stored in a segment `.bin`. */
export interface FramedChunk {
    key: boolean;
    /** Wall-clock capture time in µs (may be non-monotonic; see module docs). */
    timestampUs: number;
    data: Uint8Array;
}

const RECORD_HEADER_BYTES = 4 + 1 + 8;

/** Encode one chunk record (see the framing spec in the module docs). */
export function encodeChunkRecord(chunk: FramedChunk): Uint8Array {
    const out = new Uint8Array(RECORD_HEADER_BYTES + chunk.data.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, chunk.data.length, true);
    view.setUint8(4, chunk.key ? 1 : 0);
    view.setBigUint64(5, BigInt(Math.round(chunk.timestampUs)), true);
    out.set(chunk.data, RECORD_HEADER_BYTES);
    return out;
}

/** Decode every complete record in a segment `.bin`. A torn final record
 *  (header or payload cut short by a crash mid-write) is silently dropped:
 *  every prefix of a valid stream is a valid stream. */
export function decodeChunkRecords(bytes: Uint8Array): FramedChunk[] {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const chunks: FramedChunk[] = [];
    let off = 0;
    while (off + RECORD_HEADER_BYTES <= bytes.length) {
        const len = view.getUint32(off, true);
        if (off + RECORD_HEADER_BYTES + len > bytes.length) break; // torn tail
        chunks.push({
            key: view.getUint8(off + 4) !== 0,
            timestampUs: Number(view.getBigUint64(off + 5, true)),
            data: bytes.subarray(off + RECORD_HEADER_BYTES, off + RECORD_HEADER_BYTES + len),
        });
        off += RECORD_HEADER_BYTES + len;
    }
    return chunks;
}
