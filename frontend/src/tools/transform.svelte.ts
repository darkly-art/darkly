/**
 * Transform tool.
 *
 * When activated on a layer with content (or after paste-in-place), displays
 * interactive handles for move, scale, and rotate. Enter commits the
 * transform; Escape cancels.
 *
 * Uses the SVG ToolOverlay system for handle rendering. Affine matrix
 * computation happens in JS using the same [a, b, tx, c, d, ty] format
 * as the Rust gpu/transform.rs module.
 */
import type { Tool, ToolContext } from './registry';
import type { ToolOverlayData, OverlayHandle, OverlayLine } from '../canvas/overlay';
import { app } from '../state/app.svelte';

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

const ROTATION_ARM_LENGTH = 30; // screen pixels

/** Read floating content info from Rust and populate JS state.
 *  Called ONLY from event handlers (not from getOverlay). */
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
        const dragOffset = affineTransform(initialMatrix, dragLocal[0], dragLocal[1]);
        const anchorOffset = affineTransform(initialMatrix, anchorLocal[0], anchorLocal[1]);

        const mouseOffset: [number, number] = [canvasX - origin[0], canvasY - origin[1]];
        const dInitialX = dragOffset[0] - anchorOffset[0];
        const dInitialY = dragOffset[1] - anchorOffset[1];
        const dMouseX = mouseOffset[0] - anchorOffset[0];
        const dMouseY = mouseOffset[1] - anchorOffset[1];

        let sx = Math.abs(dInitialX) > 0.01 ? dMouseX / dInitialX : 1;
        let sy = Math.abs(dInitialY) > 0.01 ? dMouseY / dInitialY : 1;

        if (handle === Handle.Top || handle === Handle.Bottom) sx = 1;
        if (handle === Handle.Left || handle === Handle.Right) sy = 1;

        if (shiftKey && CORNER_HANDLES.includes(handle)) {
            const uniform = Math.max(Math.abs(sx), Math.abs(sy));
            sx = uniform * Math.sign(sx || 1);
            sy = uniform * Math.sign(sy || 1);
        }

        matrix = affineMultiply(
            affineTranslate(anchorOffset[0], anchorOffset[1]),
            affineMultiply(
                affineScale(sx, sy),
                affineMultiply(
                    affineTranslate(-anchorOffset[0], -anchorOffset[1]),
                    initialMatrix,
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
// Overlay generation (pure — only reads $state, no side effects)
// ---------------------------------------------------------------------------

function buildOverlay(): ToolOverlayData | null {
    if (!active) return null;

    const tl = toCanvas(0, 0);
    const tr = toCanvas(srcW, 0);
    const br = toCanvas(srcW, srcH);
    const bl = toCanvas(0, srcH);

    const tm: [number, number] = [(tl[0] + tr[0]) / 2, (tl[1] + tr[1]) / 2];
    const rm: [number, number] = [(tr[0] + br[0]) / 2, (tr[1] + br[1]) / 2];
    const bm: [number, number] = [(br[0] + bl[0]) / 2, (br[1] + bl[1]) / 2];
    const lm: [number, number] = [(bl[0] + tl[0]) / 2, (bl[1] + tl[1]) / 2];

    const edgeX = tr[0] - tl[0];
    const edgeY = tr[1] - tl[1];
    const edgeLen = Math.sqrt(edgeX * edgeX + edgeY * edgeY);
    const perpX = edgeLen > 0.01 ? edgeY / edgeLen : 0;
    const perpY = edgeLen > 0.01 ? -edgeX / edgeLen : -1;
    const armCanvas = ROTATION_ARM_LENGTH / (app.zoom || 1);
    const rotPos: [number, number] = [tm[0] + perpX * armCanvas, tm[1] + perpY * armCanvas];

    const lines: OverlayLine[] = [
        { x1: tl[0], y1: tl[1], x2: tr[0], y2: tr[1], stroke: '#4af', dashArray: '6 4' },
        { x1: tr[0], y1: tr[1], x2: br[0], y2: br[1], stroke: '#4af', dashArray: '6 4' },
        { x1: br[0], y1: br[1], x2: bl[0], y2: bl[1], stroke: '#4af', dashArray: '6 4' },
        { x1: bl[0], y1: bl[1], x2: tl[0], y2: tl[1], stroke: '#4af', dashArray: '6 4' },
        { x1: tm[0], y1: tm[1], x2: rotPos[0], y2: rotPos[1], stroke: '#4af', dashArray: '4 3' },
    ];

    const makeHandle = (id: string, pos: [number, number], handle: Handle, radius = 5): OverlayHandle => ({
        id,
        x: pos[0],
        y: pos[1],
        radius,
        cursor: cursorForHandle(handle),
        fill: '#fff',
        stroke: '#4af',
        onDrag(cx, cy) { beginDragIfNeeded(handle, cx, cy); updateDrag(cx, cy, false); },
        onDragEnd() { endDrag(); },
    });

    const handles: OverlayHandle[] = [
        makeHandle('tl', tl, Handle.TopLeft, 5),
        makeHandle('tr', tr, Handle.TopRight, 5),
        makeHandle('br', br, Handle.BottomRight, 5),
        makeHandle('bl', bl, Handle.BottomLeft, 5),
        makeHandle('tm', tm, Handle.Top, 4),
        makeHandle('rm', rm, Handle.Right, 4),
        makeHandle('bm', bm, Handle.Bottom, 4),
        makeHandle('lm', lm, Handle.Left, 4),
        { ...makeHandle('rot', rotPos, Handle.Rotate, 5), fill: '#4af', stroke: '#fff' },
    ];

    return { lines, handles };
}

function beginDragIfNeeded(handle: Handle, cx: number, cy: number) {
    if (!drag) beginDrag(handle, cx, cy);
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

    onPointerDown(_ctx, _e, cx, cy) {
        if (!active) {
            if (app.handle && app.activeLayerId != null) {
                if (!app.handle.has_floating()) {
                    app.handle.begin_transform(app.activeLayerId);
                }
                syncFromRust();
            }
            if (!active) return;
        }
        beginDrag(Handle.Body, cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        if (drag) {
            updateDrag(cx, cy, e.shiftKey);
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

    getOverlay(): ToolOverlayData | null {
        return buildOverlay();
    },

    dismissOverlay() {
        if (app.handle?.has_floating()) {
            app.handle.commit_floating();
            app.requestFrame();
        }
        clearState();
    },
};
