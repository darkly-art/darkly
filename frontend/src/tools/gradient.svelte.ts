import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';

// --- Reactive state ---

let startX = $state(0);
let startY = $state(0);
let endX = $state(0);
let endY = $state(0);
let isDrawing = $state(false);
let hasPlacement = $state(false);

// Click-vs-drag detection: when clicking on the canvas with an active
// placement, we defer starting a new gradient until a drag threshold is
// exceeded.  If the pointer releases first, we just dismiss.
const DRAG_THRESHOLD = 4; // screen pixels
let pending: { screenX: number; screenY: number; cx: number; cy: number } | null = null;

/** Which handle is being dragged ('start' | 'end'), or null. */
let draggingHandle: string | null = null;

let canvasEl: HTMLCanvasElement | null = null;
let overlay: OverlayBuilder | null = null;

function applyGradient() {
    const layerId = app.activeLayerId;
    if (!layerId || !app.engine) return;

    const c = app.foreground;
    const bg = app.background;

    app.engine.post('begin_stroke', { id: layerId });
    app.engine.post('stroke_to', {
        op: {
            op: 'linear_gradient',
            x0: startX, y0: startY,
            x1: endX, y1: endY,
            r0: c.r, g0: c.g, b0: c.b, a0: c.a,
            r1: bg.r, g1: bg.g, b1: bg.b, a1: bg.a,
        },
    });
    app.engine.post('end_stroke');
}

function clearPlacement() {
    isDrawing = false;
    hasPlacement = false;
    pending = null;
    draggingHandle = null;
    overlay = null;
    app.engine?.post('clear_overlay');
    app.toolCursor = null;
}

function beginDrawing(cx: number, cy: number) {
    startX = cx;
    startY = cy;
    endX = cx;
    endY = cy;
    isDrawing = true;
}

function buildOverlay(): OverlayBuilder | null {
    if ((!isDrawing && !hasPlacement) || !canvasEl || !app.engine) return null;

    const o = new OverlayBuilder(canvasEl);
    o.line([startX, startY], [endX, endY]);
    o.handle([startX, startY], { id: 'start', cursor: 'grab', fill: '#4af', stroke: '#fff' });
    o.handle([endX, endY],     { id: 'end',   cursor: 'grab', fill: '#fa4', stroke: '#fff' });
    o.push(app.engine);
    return o;
}

// --- Tool definition ---

// Custom icon: no icon set has anything that reads as "linear gradient" at
// toolbar size. The bespoke SVG lives at src/icons/svg/gradient.svg and is
// bundled under the `local:` prefix (see scripts/gen-icon-bundle.mjs) — a
// rounded square painted with a currentColor→transparent fade, so it inherits
// the toolbar's muted/active text color.
export const gradientTool: Tool = {
    id: 'gradient',
    icon: 'local:gradient',
    group: 'paint',
    cluster: 'fill',
    hotkeyAction: 'gradientTool',

    onActivate(ctx) {
        canvasEl = ctx.canvasEl;
    },

    onDeactivate() {
        clearPlacement();
    },

    dismissOverlay() {
        clearPlacement();
    },

    onPointerDown(_ctx, e, cx, cy) {
        if (!app.activeLayerId) return;

        // Check if clicking on an existing handle
        if (hasPlacement && overlay) {
            const hit = overlay.hitTest(cx, cy);
            if (hit) {
                draggingHandle = hit.id;
                return;
            }
        }

        if (hasPlacement) {
            // Defer: might be click-to-dismiss or drag-to-start-new
            pending = { screenX: e.clientX, screenY: e.clientY, cx, cy };
            return;
        }

        beginDrawing(cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        // Handle drag on an endpoint
        if (draggingHandle) {
            if (draggingHandle === 'start') { startX = cx; startY = cy; }
            else { endX = cx; endY = cy; }
            app.requestFrame();
            return;
        }

        if (pending) {
            const dx = e.clientX - pending.screenX;
            const dy = e.clientY - pending.screenY;
            if (dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD) {
                const start = pending;
                clearPlacement();
                beginDrawing(start.cx, start.cy);
                endX = cx;
                endY = cy;
            }
            return;
        }

        if (isDrawing) {
            endX = cx;
            endY = cy;
        } else if (hasPlacement && overlay) {
            // Hover cursor feedback
            const hit = overlay.hitTest(cx, cy);
            app.toolCursor = hit?.cursor ?? null;
        }
    },

    onPointerUp(ctx, e) {
        if (draggingHandle) {
            draggingHandle = null;
            if (hasPlacement) applyGradient();
            app.requestFrame();
            return;
        }

        if (pending) {
            clearPlacement();
            return;
        }
        if (!isDrawing) return;
        const pos = ctx.screenToCanvas(e.clientX, e.clientY);
        endX = pos.x;
        endY = pos.y;
        isDrawing = false;
        hasPlacement = true;
        applyGradient();
    },

    onFrame() {
        if (isDrawing || hasPlacement) {
            overlay = buildOverlay();
        }
    },
};
