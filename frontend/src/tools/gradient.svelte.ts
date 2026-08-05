import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';

// Click-vs-drag detection: when clicking on the canvas with an active
// placement, we defer starting a new gradient until a drag threshold is
// exceeded.  If the pointer releases first, we just dismiss.
const DRAG_THRESHOLD = 4; // screen pixels

class GradientTool extends ToolBase {
    private startX = 0;
    private startY = 0;
    private endX = 0;
    private endY = 0;
    private isDrawing = false;
    private hasPlacement = false;

    private pending: { screenX: number; screenY: number; cx: number; cy: number } | null = null;

    /** Which handle is being dragged ('start' | 'end'), or null. */
    private draggingHandle: string | null = null;

    private overlay: OverlayBuilder | null = null;

    private applyGradient(): void {
        const engine = this.engine;
        const layerId = this.inst.activeLayerId;
        if (!layerId || !engine) return;

        const c = this.inst.foreground;
        const bg = this.inst.background;

        engine.api.beginStroke({ id: layerId });
        engine.api.strokeTo({
            op: {
                op: 'linear_gradient',
                x0: this.startX, y0: this.startY,
                x1: this.endX, y1: this.endY,
                r0: c.r, g0: c.g, b0: c.b, a0: c.a,
                r1: bg.r, g1: bg.g, b1: bg.b, a1: bg.a,
            },
        });
        engine.api.endStroke();
    }

    private clearPlacement(): void {
        this.isDrawing = false;
        this.hasPlacement = false;
        this.pending = null;
        this.draggingHandle = null;
        this.overlay = null;
        this.engine?.api.clearOverlay();
        this.inst.toolCursor = null;
    }

    private beginDrawing(cx: number, cy: number): void {
        this.startX = cx;
        this.startY = cy;
        this.endX = cx;
        this.endY = cy;
        this.isDrawing = true;
    }

    private buildOverlay(): OverlayBuilder | null {
        const engine = this.engine;
        const canvasEl = this.canvasEl;
        if ((!this.isDrawing && !this.hasPlacement) || !canvasEl || !engine) return null;

        const o = new OverlayBuilder(canvasEl);
        o.line([this.startX, this.startY], [this.endX, this.endY]);
        o.handle([this.startX, this.startY], { id: 'start', cursor: 'grab', fill: '#4af', stroke: '#fff' });
        o.handle([this.endX, this.endY],     { id: 'end',   cursor: 'grab', fill: '#fa4', stroke: '#fff' });
        o.push(engine);
        return o;
    }

    onDeactivate(): void {
        this.clearPlacement();
    }

    dismissOverlay(): void {
        this.clearPlacement();
    }

    onPointerDown(e: PointerEvent, cx: number, cy: number): void {
        if (!this.inst.activeLayerId) return;

        // Check if clicking on an existing handle
        if (this.hasPlacement && this.overlay) {
            const hit = this.overlay.hitTest(cx, cy);
            if (hit) {
                this.draggingHandle = hit.id;
                return;
            }
        }

        if (this.hasPlacement) {
            // Defer: might be click-to-dismiss or drag-to-start-new
            this.pending = { screenX: e.clientX, screenY: e.clientY, cx, cy };
            return;
        }

        this.beginDrawing(cx, cy);
    }

    onPointerMove(e: PointerEvent, cx: number, cy: number): void {
        // Handle drag on an endpoint
        if (this.draggingHandle) {
            if (this.draggingHandle === 'start') { this.startX = cx; this.startY = cy; }
            else { this.endX = cx; this.endY = cy; }
            this.inst.requestFrame();
            return;
        }

        if (this.pending) {
            const dx = e.clientX - this.pending.screenX;
            const dy = e.clientY - this.pending.screenY;
            if (dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD) {
                const start = this.pending;
                this.clearPlacement();
                this.beginDrawing(start.cx, start.cy);
                this.endX = cx;
                this.endY = cy;
            }
            return;
        }

        if (this.isDrawing) {
            this.endX = cx;
            this.endY = cy;
        } else if (this.hasPlacement && this.overlay) {
            // Hover cursor feedback
            const hit = this.overlay.hitTest(cx, cy);
            this.inst.toolCursor = hit?.cursor ?? null;
        }
    }

    onPointerUp(e: PointerEvent): void {
        if (this.draggingHandle) {
            this.draggingHandle = null;
            if (this.hasPlacement) this.applyGradient();
            this.inst.requestFrame();
            return;
        }

        if (this.pending) {
            this.clearPlacement();
            return;
        }
        if (!this.isDrawing) return;
        const pos = this.screenToCanvas(e.clientX, e.clientY);
        this.endX = pos.x;
        this.endY = pos.y;
        this.isDrawing = false;
        this.hasPlacement = true;
        this.applyGradient();
    }

    onFrame(): void {
        if (this.isDrawing || this.hasPlacement) {
            this.overlay = this.buildOverlay();
        }
    }
}

// Custom icon: no icon set has anything that reads as "linear gradient" at
// toolbar size. The bespoke SVG lives at src/icons/svg/gradient.svg and is
// bundled under the `local:` prefix (see scripts/gen-icon-bundle.mjs) — a
// rounded square painted with a currentColor→transparent fade, so it inherits
// the toolbar's muted/active text color.
export const gradientTool: ToolDescriptor = {
    id: 'gradient',
    group: 'paint',
    cluster: 'fill',
    hotkeyAction: 'gradientTool',
    create: (inst: DarklyInstance) => new GradientTool(inst),
};
