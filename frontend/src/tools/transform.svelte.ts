/**
 * Transform tool.
 *
 * When activated on a layer with content (or after paste-in-place), displays
 * interactive handles for move, scale, and rotate. Enter commits the
 * transform; Escape cancels.
 *
 * Handles are rendered via the GPU overlay system (not SVG) for smooth
 * drag performance. Hit-testing for cursor feedback and drag initiation
 * is pure JS math (distance checks against known handle positions).
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';

// ---------------------------------------------------------------------------
// Affine2D helpers (mirrors Rust gpu/transform.rs)
// ---------------------------------------------------------------------------

type Affine2D = [number, number, number, number, number, number];
const IDENTITY: Affine2D = [1, 0, 0, 0, 1, 0];

function affineTransform(m: Affine2D, x: number, y: number): [number, number] {
    return [m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]];
}

function affineMultiply(a: Affine2D, b: Affine2D): Affine2D {
    return [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ];
}

function affineTranslate(tx: number, ty: number): Affine2D {
    return [1, 0, tx, 0, 1, ty];
}

function affineScale(sx: number, sy: number): Affine2D {
    return [sx, 0, 0, 0, sy, 0];
}

function affineInverse(m: Affine2D): Affine2D | null {
    const [a, b, tx, c, d, ty] = m;
    const det = a * d - b * c;
    if (Math.abs(det) < 1e-12) return null;
    const inv = 1 / det;
    return [
        d * inv,
        -b * inv,
        (b * ty - d * tx) * inv,
        -c * inv,
        a * inv,
        (c * tx - a * ty) * inv,
    ];
}

function affineRotate(angle: number): Affine2D {
    const c = Math.cos(angle);
    const s = Math.sin(angle);
    return [c, -s, 0, s, c, 0];
}

// ---------------------------------------------------------------------------
// Handle enumeration
// ---------------------------------------------------------------------------

const enum Handle {
    TopLeft, Top, TopRight, Right, BottomRight, Bottom, BottomLeft, Left,
    Rotate,
    Body,
}

const CORNER_HANDLES = [Handle.TopLeft, Handle.TopRight, Handle.BottomRight, Handle.BottomLeft];

/** Source-local coordinates for each handle. */
function handleLocal(h: Handle, w: number, ht: number): [number, number] {
    switch (h) {
        case Handle.TopLeft:     return [0, 0];
        case Handle.Top:         return [w / 2, 0];
        case Handle.TopRight:    return [w, 0];
        case Handle.Right:       return [w, ht / 2];
        case Handle.BottomRight: return [w, ht];
        case Handle.Bottom:      return [w / 2, ht];
        case Handle.BottomLeft:  return [0, ht];
        case Handle.Left:        return [0, ht / 2];
        case Handle.Rotate:      return [w / 2, ht / 2];
        case Handle.Body:        return [w / 2, ht / 2];
    }
}

/** Opposite anchor for scale operations. */
function oppositeHandle(h: Handle): Handle {
    switch (h) {
        case Handle.TopLeft:     return Handle.BottomRight;
        case Handle.Top:         return Handle.Bottom;
        case Handle.TopRight:    return Handle.BottomLeft;
        case Handle.Right:       return Handle.Left;
        case Handle.BottomRight: return Handle.TopLeft;
        case Handle.Bottom:      return Handle.Top;
        case Handle.BottomLeft:  return Handle.TopRight;
        case Handle.Left:        return Handle.Right;
        default:                 return Handle.Body;
    }
}

function cursorForHandle(h: Handle): string {
    switch (h) {
        case Handle.TopLeft:
        case Handle.BottomRight: return 'nwse-resize';
        case Handle.TopRight:
        case Handle.BottomLeft:  return 'nesw-resize';
        case Handle.Top:
        case Handle.Bottom:      return 'ns-resize';
        case Handle.Left:
        case Handle.Right:       return 'ew-resize';
        case Handle.Rotate:      return ROTATE_CURSOR;
        case Handle.Body:        return 'move';
    }
}

/**
 * Rotation cursor used when hovering outside the transform bounding box
 * (matches Krita's free-transform behavior). Browsers have no standard
 * rotation cursor, so we use an inline SVG.
 */
const ROTATE_CURSOR =
    "url(\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'><path d='M21 12a9 9 0 1 1-3.5-7.1'/><polyline points='21 3 21 9 15 9'/></svg>\") 12 12, grab";

// ---------------------------------------------------------------------------
// Reactive tool state
// ---------------------------------------------------------------------------

/** Whether the tool is currently active with floating content. */
let active = $state(false);

/** Current affine matrix (JS-side state, pushed to Rust on change). */
let matrix = $state<Affine2D>([...IDENTITY]);

/** Source origin in canvas space. */
let origin = $state<[number, number]>([0, 0]);

/** Source dimensions. */
let srcW = $state(0);
let srcH = $state(0);

/** Active drag state. */
let drag = $state<{
    handle: Handle;
    initialMatrix: Affine2D;
    startCanvas: [number, number];
    anchorLocal: [number, number];
    anchorCanvas: [number, number];
    centerCanvas: [number, number];
    startAngle: number;
} | null>(null);

/** Canvas element reference for coordinate conversions. */
let canvasEl: HTMLCanvasElement | null = null;

/**
 * Current transformed bounding box in canvas space (tl, tr, br, bl).
 * Updated each frame by buildOverlay(); used by pointer handlers to
 * decide whether a point is inside (→ move) or outside (→ rotate).
 */
let bboxPolygon: [
    [number, number], [number, number], [number, number], [number, number],
] | null = null;

/** Ray-casting point-in-polygon test. */
function pointInPolygon(
    px: number, py: number,
    poly: readonly [number, number][],
): boolean {
    let inside = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
        const [xi, yi] = poly[i];
        const [xj, yj] = poly[j];
        if (((yi > py) !== (yj > py)) &&
            (px < ((xj - xi) * (py - yi)) / (yj - yi) + xi)) {
            inside = !inside;
        }
    }
    return inside;
}

/**
 * Resolve which Handle a canvas-space point corresponds to, using Krita-style
 * priority: a handle within its hit radius wins; otherwise the bounding-box
 * polygon partitions into Body (inside) vs Rotate (outside).
 */
function resolveHandle(canvasX: number, canvasY: number): {
    id: Handle; cursor: string;
} {
    const hit = overlay?.hitTest(canvasX, canvasY);
    if (hit) return { id: hit.id as Handle, cursor: hit.cursor };
    if (bboxPolygon && pointInPolygon(canvasX, canvasY, bboxPolygon)) {
        return { id: Handle.Body, cursor: cursorForHandle(Handle.Body) };
    }
    return { id: Handle.Rotate, cursor: cursorForHandle(Handle.Rotate) };
}

/** Read floating content info from Rust and populate JS state.
 *  Called ONLY from event handlers (not from overlay push). */
function syncFromRust(): boolean {
    if (!app.handle) return false;
    const raw = app.handle.floating_info();
    if (!raw) {
        active = false;
        return false;
    }
    origin = [raw[0], raw[1]];
    srcW = raw[2];
    srcH = raw[3];
    matrix = [raw[4], raw[5], raw[6], raw[7], raw[8], raw[9]];
    active = true;
    return true;
}

function clearState() {
    active = false;
    drag = null;
    bboxPolygon = null;
    app.handle?.clear_overlay();
    app.toolCursor = null;
}

function pushMatrix() {
    app.handle?.update_floating_matrix(new Float32Array(matrix));
    app.requestFrame();
}

/** Convert a source-local point to canvas space using current matrix + origin. */
function toCanvas(lx: number, ly: number): [number, number] {
    const [cx, cy] = affineTransform(matrix, lx, ly);
    return [cx + origin[0], cy + origin[1]];
}

// ---------------------------------------------------------------------------
// Drag mechanics
// ---------------------------------------------------------------------------

function beginDrag(handle: Handle, canvasX: number, canvasY: number) {
    const initialMatrix: Affine2D = [...matrix];
    const startCanvas: [number, number] = [canvasX, canvasY];

    const anchorLocal = handleLocal(oppositeHandle(handle), srcW, srcH);
    const anchorCanvas = toCanvas(anchorLocal[0], anchorLocal[1]);

    const centerCanvas = toCanvas(srcW / 2, srcH / 2);
    const startAngle = Math.atan2(canvasY - centerCanvas[1], canvasX - centerCanvas[0]);

    drag = { handle, initialMatrix, startCanvas, anchorLocal, anchorCanvas, centerCanvas, startAngle };
}

function updateDrag(canvasX: number, canvasY: number, shiftKey: boolean) {
    if (!drag) return;

    const { handle, initialMatrix, startCanvas, anchorLocal, anchorCanvas, centerCanvas, startAngle } = drag;

    if (handle === Handle.Body) {
        const dx = canvasX - startCanvas[0];
        const dy = canvasY - startCanvas[1];
        matrix = affineMultiply(affineTranslate(dx, dy), initialMatrix);
    } else if (handle === Handle.Rotate) {
        let angle = Math.atan2(canvasY - centerCanvas[1], canvasX - centerCanvas[0]) - startAngle;
        if (shiftKey) {
            const snap = Math.PI / 12;
            angle = Math.round(angle / snap) * snap;
        }
        const cLocal: [number, number] = [srcW / 2, srcH / 2];
        const cOffset = affineTransform(initialMatrix, cLocal[0], cLocal[1]);
        matrix = affineMultiply(
            affineTranslate(cOffset[0], cOffset[1]),
            affineMultiply(
                affineRotate(angle),
                affineMultiply(
                    affineTranslate(-cOffset[0], -cOffset[1]),
                    initialMatrix,
                ),
            ),
        );
    } else {
        const dragLocal = handleLocal(handle, srcW, srcH);
        const mouseOffset: [number, number] = [canvasX - origin[0], canvasY - origin[1]];

        const inv = affineInverse(initialMatrix);
        if (!inv) return;
        const mouseLocal = affineTransform(inv, mouseOffset[0], mouseOffset[1]);

        const dLocalX = dragLocal[0] - anchorLocal[0];
        const dLocalY = dragLocal[1] - anchorLocal[1];
        const dMouseLocalX = mouseLocal[0] - anchorLocal[0];
        const dMouseLocalY = mouseLocal[1] - anchorLocal[1];

        let sx = Math.abs(dLocalX) > 0.01 ? dMouseLocalX / dLocalX : 1;
        let sy = Math.abs(dLocalY) > 0.01 ? dMouseLocalY / dLocalY : 1;

        if (handle === Handle.Top || handle === Handle.Bottom) sx = 1;
        if (handle === Handle.Left || handle === Handle.Right) sy = 1;

        if (shiftKey && CORNER_HANDLES.includes(handle)) {
            const uniform = Math.max(Math.abs(sx), Math.abs(sy));
            sx = uniform * Math.sign(sx || 1);
            sy = uniform * Math.sign(sy || 1);
        }

        matrix = affineMultiply(
            initialMatrix,
            affineMultiply(
                affineTranslate(anchorLocal[0], anchorLocal[1]),
                affineMultiply(
                    affineScale(sx, sy),
                    affineTranslate(-anchorLocal[0], -anchorLocal[1]),
                ),
            ),
        );
    }

    pushMatrix();
}

function endDrag() {
    drag = null;
}

// ---------------------------------------------------------------------------
// GPU overlay (built via OverlayBuilder, rendered by GPU, hit-tested by JS)
// ---------------------------------------------------------------------------

/** Last-built overlay — kept for hit-testing between frames. */
let overlay: OverlayBuilder | null = null;

function mid(a: [number, number], b: [number, number]): [number, number] {
    return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}

function buildOverlay(): OverlayBuilder | null {
    if (!active || !app.handle || !canvasEl) {
        app.handle?.clear_overlay();
        bboxPolygon = null;
        return null;
    }

    const o = new OverlayBuilder(canvasEl);

    // Corner positions in canvas space
    const tl = toCanvas(0, 0);
    const tr = toCanvas(srcW, 0);
    const br = toCanvas(srcW, srcH);
    const bl = toCanvas(0, srcH);
    bboxPolygon = [tl, tr, br, bl];

    // Edge midpoints
    const tm = mid(tl, tr);
    const rm = mid(tr, br);
    const bm = mid(br, bl);
    const lm = mid(bl, tl);

    // Bounding box
    o.line(tl, tr, { color: '#4af', dash: 6 });
    o.line(tr, br, { color: '#4af', dash: 6 });
    o.line(br, bl, { color: '#4af', dash: 6 });
    o.line(bl, tl, { color: '#4af', dash: 6 });

    // Corner handles
    o.handle(tl, { id: Handle.TopLeft,     cursor: 'nwse-resize' });
    o.handle(tr, { id: Handle.TopRight,    cursor: 'nesw-resize' });
    o.handle(br, { id: Handle.BottomRight, cursor: 'nwse-resize' });
    o.handle(bl, { id: Handle.BottomLeft,  cursor: 'nesw-resize' });

    // Edge handles
    o.handle(tm, { id: Handle.Top,    cursor: 'ns-resize', radius: 4 });
    o.handle(rm, { id: Handle.Right,  cursor: 'ew-resize', radius: 4 });
    o.handle(bm, { id: Handle.Bottom, cursor: 'ns-resize', radius: 4 });
    o.handle(lm, { id: Handle.Left,   cursor: 'ew-resize', radius: 4 });

    o.push(app.handle);
    return o;
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

export const transformTool: Tool = {
    id: 'transform',
    name: 'Transform',
    faIcon: 'fa-solid fa-up-down-left-right',
    group: 'transform',
    hotkeyAction: 'transformTool',

    onActivate(ctx) {
        canvasEl = ctx.canvasEl;
        if (!app.activeLayerId || !ctx.handle) return;
        if (!ctx.handle.has_floating()) {
            ctx.handle.begin_transform(app.activeLayerId);
        }
        syncFromRust();
        app.requestFrame();
    },

    onDeactivate(ctx) {
        if (ctx.handle?.has_floating()) {
            ctx.handle.commit_floating();
            app.requestFrame();
        }
        clearState();
    },

    onPointerDown(ctx, _e, cx, cy) {
        if (!active) {
            if (app.handle && app.activeLayerId != null) {
                if (!app.handle.has_floating()) {
                    app.handle.begin_transform(app.activeLayerId);
                }
                syncFromRust();
            }
            if (!active) return;
        }
        beginDrag(resolveHandle(cx, cy).id, cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        if (drag) {
            updateDrag(cx, cy, e.shiftKey);
        } else if (active) {
            app.toolCursor = resolveHandle(cx, cy).cursor;
        }
    },

    onPointerUp(_ctx, _e) {
        endDrag();
    },

    onKeyDown(e) {
        if (e.key === 'Enter') {
            app.handle?.commit_floating();
            clearState();
            app.requestFrame();
            return true;
        }
        if (e.key === 'Escape') {
            app.handle?.cancel_floating();
            clearState();
            app.requestFrame();
            return true;
        }
        return false;
    },

    onFrame() {
        // Sync when floating content arrives from an async GPU readback
        // (begin_transform without selection computes content bounds async).
        if (!active && app.handle?.has_floating()) {
            syncFromRust();
        }
        if (active) {
            overlay = buildOverlay();
        }
    },

    dismissOverlay() {
        if (app.handle?.has_floating()) {
            app.handle.commit_floating();
            app.requestFrame();
        }
        clearState();
    },
};
