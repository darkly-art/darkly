/**
 * Timelapse export options — pure helpers behind the export modal and the
 * export pipeline: aspect-ratio grouping of recorded segments, the
 * stretch / fit / fill conversion geometry, aspect-locked resolution
 * derivation, and input sanitization. No DOM, no WebCodecs: everything
 * here is unit-testable in node.
 *
 * A recording holds one segment per encoder run; the document's aspect
 * ratio can change between runs (canvas resize / crop). Export renders to
 * a single output canvas, so the user picks a target aspect ratio (one of
 * those present in the recording) and how segments of *other* aspect
 * ratios are converted onto it. Grouping uses the true document canvas
 * dims stored per segment — encoder dims are even-aligned fits whose
 * ratio is perturbed at small sizes.
 */
import { decoderConfigsCompatible, type SegmentMeta } from './segments';

/** Per-axis ceiling for the output resolution: H.264 High 5.2 tops out at
 *  4096×2304 and the codec probe settles anything finer. */
export const EXPORT_MAX_DIM = 4096;
export const EXPORT_MIN_DIM = 2;

export const EXPORT_MIN_FPS = 1;
export const EXPORT_MAX_FPS = 120;

/** Default GIF long edge — a size suggestion the modal seeds, not a cap. */
export const GIF_LONG_EDGE = 480;

// ---------------------------------------------------------------------------
// Conversion geometry
// ---------------------------------------------------------------------------

/** How segments whose aspect ratio differs from the output's are drawn:
 *  distorted to cover (`stretch`), letterboxed on black (`fit`), or
 *  center-cropped to cover (`fill`). */
export type ConversionMethod = 'stretch' | 'fit' | 'fill';

/** The `drawImage` destination rect for one source frame on the output
 *  canvas. `fill` overflows the canvas (the 2D context clips); a source
 *  matching the output aspect fills the canvas exactly under all three
 *  methods, so the pipeline applies the chosen method uniformly. */
export function computeDrawRect(
    method: ConversionMethod,
    srcW: number,
    srcH: number,
    outW: number,
    outH: number,
): { x: number; y: number; w: number; h: number } {
    if (method === 'stretch') return { x: 0, y: 0, w: outW, h: outH };
    const sx = outW / srcW;
    const sy = outH / srcH;
    const scale = method === 'fit' ? Math.min(sx, sy) : Math.max(sx, sy);
    const w = srcW * scale;
    const h = srcH * scale;
    return { x: (outW - w) / 2, y: (outH - h) / 2, w, h };
}

// ---------------------------------------------------------------------------
// Aspect-ratio grouping
// ---------------------------------------------------------------------------

/** All segments sharing one document aspect ratio, summarized for the
 *  export modal's aspect-ratio choice. */
export interface AspectGroup {
    /** The aspect ratio as a reduced integer fraction of the canvas dims. */
    arW: number;
    arH: number;
    /** Human label: a named ratio (`16:9`), small exact terms, or decimal. */
    label: string;
    frameCount: number;
    /** Encoder dims of the group's largest segment — the native output
     *  resolution offered as the default. */
    nativeWidth: number;
    nativeHeight: number;
}

function gcd(a: number, b: number): number {
    while (b !== 0) [a, b] = [b, a % b];
    return a;
}

/** Group segments by exact document aspect ratio, in order of first
 *  appearance. `defaultIndex` is the last segment's group — the document's
 *  final shape, the likeliest export target. */
export function groupSegmentsByAspect(metas: SegmentMeta[]): {
    groups: AspectGroup[];
    defaultIndex: number;
} {
    const groups: AspectGroup[] = [];
    const byKey = new Map<string, number>();
    let defaultIndex = 0;
    for (const meta of metas) {
        const d = gcd(meta.canvasWidth, meta.canvasHeight);
        const arW = meta.canvasWidth / d;
        const arH = meta.canvasHeight / d;
        const key = `${arW}:${arH}`;
        let index = byKey.get(key);
        if (index === undefined) {
            index = groups.length;
            byKey.set(key, index);
            groups.push({
                arW,
                arH,
                label: aspectLabel(arW, arH),
                frameCount: 0,
                nativeWidth: meta.width,
                nativeHeight: meta.height,
            });
        }
        const group = groups[index];
        group.frameCount += meta.frameCount;
        if (meta.width * meta.height > group.nativeWidth * group.nativeHeight) {
            group.nativeWidth = meta.width;
            group.nativeHeight = meta.height;
        }
        defaultIndex = index;
    }
    return { groups, defaultIndex };
}

const NAMED_RATIOS: Array<[number, number]> = [
    [1, 1],
    [4, 3],
    [3, 2],
    [16, 9],
    [21, 9],
];

/** Label a reduced aspect fraction: the nearest named ratio (within 1%,
 *  covering even-align perturbation and odd-pixel crops), exact small
 *  terms, or a decimal like `1.85:1`. */
export function aspectLabel(arW: number, arH: number): string {
    const ar = arW / arH;
    for (const [w, h] of NAMED_RATIOS) {
        if (Math.abs(ar - w / h) / (w / h) < 0.01) return `${w}:${h}`;
        if (Math.abs(ar - h / w) / (h / w) < 0.01) return `${h}:${w}`;
    }
    if (arW <= 30 && arH <= 30) return `${arW}:${arH}`;
    return ar >= 1 ? `${ar.toFixed(2)}:1` : `1:${(1 / ar).toFixed(2)}`;
}

// ---------------------------------------------------------------------------
// Input sanitization
// ---------------------------------------------------------------------------

function clampDim(v: number): number {
    if (!Number.isFinite(v)) return EXPORT_MIN_DIM;
    const even = Math.round(v) & ~1;
    return Math.min(EXPORT_MAX_DIM, Math.max(EXPORT_MIN_DIM, even));
}

/** Derive both output dims from one edited axis, locked to the aspect
 *  ratio: the edited value is sanitized (even-aligned, clamped to
 *  [2, `EXPORT_MAX_DIM`]) and the other axis follows the exact fraction. */
export function lockedDims(
    axis: 'w' | 'h',
    value: number,
    arW: number,
    arH: number,
): { width: number; height: number } {
    const v = clampDim(value);
    return axis === 'w'
        ? { width: v, height: clampDim((v * arH) / arW) }
        : { width: clampDim((v * arW) / arH), height: v };
}

/** Sanitize the playback-rate input: finite, clamped to
 *  [`EXPORT_MIN_FPS`, `EXPORT_MAX_FPS`]; fractional rates are allowed. */
export function clampFps(v: number): number {
    if (!Number.isFinite(v)) return 30;
    return Math.min(EXPORT_MAX_FPS, Math.max(EXPORT_MIN_FPS, v));
}

/** Per-frame GIF delay for a playback rate. GIF stores delays in
 *  centiseconds and browsers snap sub-20ms delays to ~100ms, so the
 *  effective GIF ceiling is 50 fps; MP4 alone honors arbitrary rates. */
export function gifDelayMs(fps: number): number {
    return Math.max(20, 1000 / fps);
}

// ---------------------------------------------------------------------------
// Pipeline eligibility
// ---------------------------------------------------------------------------

/** True when the encoded packets can pass straight through to the muxer:
 *  every segment shares one decoder config and the output resolution is
 *  exactly the packets' — a container can't rescale. The playback rate
 *  never matters (packets are re-stamped either way). */
export function canPassthrough(metas: SegmentMeta[], outW: number, outH: number): boolean {
    if (metas.length === 0) return false;
    const first = metas[0];
    return (
        first.width === outW &&
        first.height === outH &&
        metas.every((m) => decoderConfigsCompatible(first, m))
    );
}
