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
import { canvasToScreen } from '../canvas/coordinates';
import {
    KIND_DASHED_LINE, KIND_FILLED_CIRCLE, KIND_CIRCLE,
    FLAG_CANVAS_SPACE, prim,
    type GpuPrim,
} from './selection_helpers';

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
        case Handle.Rotate:      return [w / 2, 0];
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
        case Handle.Rotate:      return 'grab';
        case Handle.Body:        return 'move';
    }
}

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

const ROTATION_ARM_LENGTH = 30; // screen pixels

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
// GPU overlay primitives
// ---------------------------------------------------------------------------

const LINE_COLOR: [number, number, number, number] = [0.267, 0.667, 1.0, 1.0]; // #4af
const WHITE: [number, number, number, number] = [1, 1, 1, 1];

function pushOverlayPrimitives() {
    if (!active || !app.handle || !canvasEl) {
        app.handle?.clear_overlay();
        return;
    }

    const dpr = window.devicePixelRatio || 1;
    const prims: GpuPrim[] = [];

    // Corner positions in canvas space
    const tl = toCanvas(0, 0);
    const tr = toCanvas(srcW, 0);
    const br = toCanvas(srcW, srcH);
    const bl = toCanvas(0, srcH);

    // Edge midpoints
    const tm = mid(tl, tr);
    const rm = mid(tr, br);
    const bm = mid(br, bl);
    const lm = mid(bl, tl);

    // Rotation handle position (perpendicular to top edge, in canvas space)
    const edgeX = tr[0] - tl[0];
    const edgeY = tr[1] - tl[1];
    const edgeLen = Math.sqrt(edgeX * edgeX + edgeY * edgeY);
    const perpX = edgeLen > 0.01 ? edgeY / edgeLen : 0;
    const perpY = edgeLen > 0.01 ? -edgeX / edgeLen : -1;
    const armCanvas = ROTATION_ARM_LENGTH / (app.zoom || 1);
    const rotPos: [number, number] = [tm[0] + perpX * armCanvas, tm[1] + perpY * armCanvas];

    // --- Lines (canvas space, transformed by shader) ---
    const lineOpts = { color: LINE_COLOR, thickness: 1, dashLen: 6 };
    prims.push(prim(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, tl, tr, lineOpts));
    prims.push(prim(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, tr, br, lineOpts));
    prims.push(prim(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, br, bl, lineOpts));
    prims.push(prim(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, bl, tl, lineOpts));
    prims.push(prim(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, tm, rotPos,
        { color: LINE_COLOR, thickness: 1, dashLen: 4 }));

    // --- Handle circles (screen space, constant pixel size) ---
    const handles: { pos: [number, number]; radius: number; fill: [number, number, number, number]; stroke: [number, number, number, number] }[] = [
        // Corners (r=5)
        { pos: tl, radius: 5, fill: WHITE, stroke: LINE_COLOR },
        { pos: tr, radius: 5, fill: WHITE, stroke: LINE_COLOR },
        { pos: br, radius: 5, fill: WHITE, stroke: LINE_COLOR },
        { pos: bl, radius: 5, fill: WHITE, stroke: LINE_COLOR },
        // Edge midpoints (r=4)
        { pos: tm, radius: 4, fill: WHITE, stroke: LINE_COLOR },
        { pos: rm, radius: 4, fill: WHITE, stroke: LINE_COLOR },
        { pos: bm, radius: 4, fill: WHITE, stroke: LINE_COLOR },
        { pos: lm, radius: 4, fill: WHITE, stroke: LINE_COLOR },
        // Rotation (r=5, colors swapped)
        { pos: rotPos, radius: 5, fill: LINE_COLOR, stroke: WHITE },
    ];

    for (const h of handles) {
        const sp = canvasToScreen(h.pos[0], h.pos[1], canvasEl);
        // canvasToScreen returns CSS pixels; overlay shader works in buffer pixels
        const center: [number, number] = [sp.x * dpr, sp.y * dpr];
        const r: [number, number] = [h.radius * dpr, 0];
        prims.push(prim(KIND_FILLED_CIRCLE, 0, center, r, { color: h.fill }));
        prims.push(prim(KIND_CIRCLE, 0, center, r, { color: h.stroke, thickness: 1.5 * dpr }));
    }

    app.handle.set_overlay(prims);
}

// ---------------------------------------------------------------------------
// JS hit-testing (distance checks in screen space)
// ---------------------------------------------------------------------------

const HIT_THRESHOLD = 10; // CSS pixels

function hitTestHandles(canvasX: number, canvasY: number): Handle | null {
    if (!active || !canvasEl) return null;

    const sp = canvasToScreen(canvasX, canvasY, canvasEl);
    const sx = sp.x;
    const sy = sp.y;

    // Corner positions in canvas space
    const tl = toCanvas(0, 0);
    const tr = toCanvas(srcW, 0);
    const br = toCanvas(srcW, srcH);
    const bl = toCanvas(0, srcH);

    // Rotation handle
    const tm = mid(tl, tr);
    const edgeX = tr[0] - tl[0];
    const edgeY = tr[1] - tl[1];
    const edgeLen = Math.sqrt(edgeX * edgeX + edgeY * edgeY);
    const perpX = edgeLen > 0.01 ? edgeY / edgeLen : 0;
    const perpY = edgeLen > 0.01 ? -edgeX / edgeLen : -1;
    const armCanvas = ROTATION_ARM_LENGTH / (app.zoom || 1);
    const rotPos: [number, number] = [tm[0] + perpX * armCanvas, tm[1] + perpY * armCanvas];

    // Test in priority order: rotation, corners, edges
    if (hitScreen(sx, sy, rotPos)) return Handle.Rotate;

    if (hitScreen(sx, sy, tl)) return Handle.TopLeft;
    if (hitScreen(sx, sy, tr)) return Handle.TopRight;
    if (hitScreen(sx, sy, br)) return Handle.BottomRight;
    if (hitScreen(sx, sy, bl)) return Handle.BottomLeft;

    const rm = mid(tr, br);
    const bm = mid(br, bl);
    const lm = mid(bl, tl);

    if (hitScreen(sx, sy, tm)) return Handle.Top;
    if (hitScreen(sx, sy, rm)) return Handle.Right;
    if (hitScreen(sx, sy, bm)) return Handle.Bottom;
    if (hitScreen(sx, sy, lm)) return Handle.Left;

    return null;
}

/** Test if screen point (sx, sy) in CSS pixels is within threshold of a canvas-space point. */
function hitScreen(sx: number, sy: number, canvasPos: [number, number]): boolean {
    const hp = canvasToScreen(canvasPos[0], canvasPos[1], canvasEl!);
    const dx = sx - hp.x;
    const dy = sy - hp.y;
    return dx * dx + dy * dy < HIT_THRESHOLD * HIT_THRESHOLD;
}

function mid(a: [number, number], b: [number, number]): [number, number] {
    return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

export const transformTool: Tool = {
    id: 'transform',
    name: 'Transform',
    icon: 'T',
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
        const hit = hitTestHandles(cx, cy);
        beginDrag(hit ?? Handle.Body, cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        if (drag) {
            updateDrag(cx, cy, e.shiftKey);
        } else if (active) {
            const hit = hitTestHandles(cx, cy);
            app.toolCursor = hit != null ? cursorForHandle(hit) : 'move';
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
            pushOverlayPrimitives();
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
