import type { Tool, ToolContext } from './registry';
import type { ToolOverlayData } from '../canvas/overlay';
import { app } from '../state/app.svelte';

export const GRADIENT_HOTKEY = 'KeyG';

// --- Reactive overlay state ---

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

function applyGradient() {
    const layerId = app.activeLayerId;
    if (!layerId || !app.handle) return;

    const c = app.foreground;
    const bg = app.background;

    app.handle.begin_stroke(BigInt(layerId));
    app.handle.stroke_to('linear_gradient', {
        x0: startX, y0: startY,
        x1: endX, y1: endY,
        r0: c.r, g0: c.g, b0: c.b, a0: c.a,
        r1: bg.r, g1: bg.g, b1: bg.b, a1: bg.a,
    });
    app.handle.end_stroke();
}

function clearPlacement() {
    isDrawing = false;
    hasPlacement = false;
    pending = null;
}

function beginDrawing(cx: number, cy: number) {
    startX = cx;
    startY = cy;
    endX = cx;
    endY = cy;
    isDrawing = true;
}

// --- Tool definition ---

export const gradientTool: Tool = {
    id: 'gradient',
    name: 'Gradient',
    icon: 'G',
    hotkeyAction: 'gradientTool',

    onDeactivate() {
        clearPlacement();
    },

    dismissOverlay() {
        clearPlacement();
    },

    onPointerDown(_ctx, e, cx, cy) {
        if (!app.activeLayerId) return;

        if (hasPlacement) {
            // Defer: might be click-to-dismiss or drag-to-start-new
            pending = { screenX: e.clientX, screenY: e.clientY, cx, cy };
            return;
        }

        beginDrawing(cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        if (pending) {
            const dx = e.clientX - pending.screenX;
            const dy = e.clientY - pending.screenY;
            if (dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD) {
                // Exceeded threshold — start a new gradient from the pending position
                const start = pending;
                clearPlacement();
                beginDrawing(start.cx, start.cy);
                endX = cx;
                endY = cy;
            }
            return;
        }
        if (!isDrawing) return;
        endX = cx;
        endY = cy;
    },

    onPointerUp(ctx, e) {
        if (pending) {
            // Click without drag — dismiss
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

    getOverlay(): ToolOverlayData | null {
        if (!isDrawing && !hasPlacement) return null;

        return {
            lines: [
                { x1: startX, y1: startY, x2: endX, y2: endY },
            ],
            handles: [
                {
                    id: 'gradient-start',
                    x: startX, y: startY,
                    fill: '#4af', stroke: '#fff',
                    onDrag(cx, cy) { startX = cx; startY = cy; },
                    onDragEnd() { if (hasPlacement) applyGradient(); },
                },
                {
                    id: 'gradient-end',
                    x: endX, y: endY,
                    fill: '#fa4', stroke: '#fff',
                    onDrag(cx, cy) { endX = cx; endY = cy; },
                    onDragEnd() { if (hasPlacement) applyGradient(); },
                },
            ],
        };
    },
};
