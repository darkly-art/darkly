// Pure geometry for the interactive canvas-resize preview.
//
// The whole interaction has a single source of truth: the **new canvas window
// expressed in content space**, a `Rect { x, y, w, h }` where the current
// document content occupies `[0..docW] × [0..docH]`. `(x, y)` is the new
// window's top-left relative to the content's top-left (it may be negative when
// growing, or positive when cropping inward). The modal converts this to a
// plane-space rect at apply time: `origin = canvas_origin + (x, y)`.
//
// Everything here is DOM-free so it can be unit-tested in the vitest node env.

export interface Rect {
    x: number;
    y: number;
    w: number;
    h: number;
}

/** 8 edge/corner handles plus the body (whole-frame translate). */
export type Handle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w' | 'body';

export const HANDLES: Handle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

export const MIN_DIM = 1;
export const MAX_DIM = 8192;

/** Round to an integer pixel count, clamped to the engine's allowed range. */
export function clampDim(v: number): number {
    return Math.max(MIN_DIM, Math.min(MAX_DIM, Math.round(v)));
}

/**
 * Content-space new-canvas rect for a 9-point anchor. `anchorX/anchorY` are the
 * fraction of the size delta taken off the top/left edge: content offset within
 * the new canvas is `(w - docW) * anchorX`, so the window's top-left sits at
 * `x = (docW - w) * anchorX`. Matches the retired engine `resize_anchor_rect`.
 */
export function rectFromAnchor(
    docW: number,
    docH: number,
    w: number,
    h: number,
    anchorX: number,
    anchorY: number,
): Rect {
    const cw = clampDim(w);
    const ch = clampDim(h);
    // `+ 0` normalizes a `-0` (e.g. `-40 * 0`) to `+0`.
    return {
        x: Math.round((docW - cw) * anchorX) + 0,
        y: Math.round((docH - ch) * anchorY) + 0,
        w: cw,
        h: ch,
    };
}

/**
 * If the rect's offset on an axis lands on a 9-point anchor (0, 0.5, 1) within
 * `tolPx`, return it; otherwise null. Used only to highlight the anchor grid:
 * a dimension with zero delta is anchor-ambiguous and yields null.
 */
export function matchedAnchor(
    docW: number,
    docH: number,
    rect: Rect,
    tolPx = 0.5,
): { ax: number | null; ay: number | null } {
    const axisMatch = (delta: number, offset: number): number | null => {
        if (delta === 0) return null;
        const frac = offset / delta;
        for (const a of [0, 0.5, 1]) {
            if (Math.abs(frac - a) * Math.abs(delta) <= tolPx) return a;
        }
        return null;
    };
    return {
        ax: axisMatch(docW - rect.w, rect.x),
        ay: axisMatch(docH - rect.h, rect.y),
    };
}

const movesLeft = (h: Handle) => h === 'w' || h === 'nw' || h === 'sw';
const movesRight = (h: Handle) => h === 'e' || h === 'ne' || h === 'se';
const movesTop = (h: Handle) => h === 'n' || h === 'nw' || h === 'ne';
const movesBottom = (h: Handle) => h === 's' || h === 'sw' || h === 'se';

/**
 * Apply a drag of `(dx, dy)` **content-space** pixels (from the rect captured at
 * pointer-down) for the given handle. Edges move one side, corners move two, the
 * body translates the whole frame. The opposite edge stays fixed. `lockAspect`
 * (corners only) preserves the start rect's aspect ratio about the fixed corner.
 * No snapping. Result is integer-pixel and clamped to `[MIN_DIM, MAX_DIM]`.
 */
export function applyDrag(
    start: Rect,
    handle: Handle,
    dx: number,
    dy: number,
    lockAspect = false,
): Rect {
    if (handle === 'body') {
        return { x: Math.round(start.x + dx), y: Math.round(start.y + dy), w: start.w, h: start.h };
    }

    let left = start.x;
    let top = start.y;
    let right = start.x + start.w;
    let bottom = start.y + start.h;

    const mL = movesLeft(handle);
    const mR = movesRight(handle);
    const mT = movesTop(handle);
    const mB = movesBottom(handle);

    if (mL) left = Math.min(start.x + dx, right - MIN_DIM);
    if (mR) right = Math.max(start.x + start.w + dx, left + MIN_DIM);
    if (mT) top = Math.min(start.y + dy, bottom - MIN_DIM);
    if (mB) bottom = Math.max(start.y + start.h + dy, top + MIN_DIM);

    let w = right - left;
    let h = bottom - top;

    const isCorner = (mL || mR) && (mT || mB);
    if (lockAspect && isCorner && start.w > 0 && start.h > 0) {
        const ratio = start.w / start.h;
        // Drive by whichever axis grew/shrank more, derive the other.
        if (w / start.w >= h / start.h) {
            h = w / ratio;
        } else {
            w = h * ratio;
        }
    }

    const cw = clampDim(w);
    const ch = clampDim(h);
    // Recompute the origin from the FIXED edge so it stays put through clamping.
    const x = mL ? right - cw : left;
    const y = mT ? bottom - ch : top;
    return { x: Math.round(x), y: Math.round(y), w: cw, h: ch };
}

/** Content→preview-pixel mapping (uniform scale + offset). */
export interface Fit {
    scale: number;
    offsetX: number;
    offsetY: number;
}

/**
 * Fit the union of the content rect `[0,0,docW,docH]` and the new-canvas `rect`
 * (plus a margin) into a `previewW × previewH` box, centered. Hold the returned
 * Fit fixed for the duration of a drag so the view doesn't rubber-band, and
 * recompute it on release / numeric edits.
 */
export function computeFit(
    docW: number,
    docH: number,
    rect: Rect,
    previewW: number,
    previewH: number,
    marginFrac = 0.12,
): Fit {
    const minX = Math.min(0, rect.x);
    const minY = Math.min(0, rect.y);
    const maxX = Math.max(docW, rect.x + rect.w);
    const maxY = Math.max(docH, rect.y + rect.h);
    const unionW = Math.max(1, maxX - minX);
    const unionH = Math.max(1, maxY - minY);
    const pad = marginFrac * Math.max(unionW, unionH);
    const paddedW = unionW + 2 * pad;
    const paddedH = unionH + 2 * pad;
    const scale = Math.min(previewW / paddedW, previewH / paddedH);
    const offsetX = (previewW - paddedW * scale) / 2 - (minX - pad) * scale;
    const offsetY = (previewH - paddedH * scale) / 2 - (minY - pad) * scale;
    return { scale, offsetX, offsetY };
}

/** Map a content-space point to preview pixels. */
export function toPreview(fit: Fit, cx: number, cy: number): [number, number] {
    return [fit.offsetX + cx * fit.scale, fit.offsetY + cy * fit.scale];
}

/** Map a preview-pixel point back to content space. */
export function toContent(fit: Fit, px: number, py: number): [number, number] {
    return [(px - fit.offsetX) / fit.scale, (py - fit.offsetY) / fit.scale];
}
