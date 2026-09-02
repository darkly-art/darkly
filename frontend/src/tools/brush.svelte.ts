import { ToolBase, type ToolDescriptor } from './registry';
import { getActiveInstance, type DarklyInstance, type Color } from '../state/app.svelte';
import { runHook } from './tool_session';
import { beginPaintStroke } from './paint_stroke';
import { brushGraph } from '../state/brush_graph.svelte';
import { effectivePressure } from '../lib/pressure';
import { strokeRecorder, currentCanvasDimensions } from '../lib/strokeRecorder';
import {
    KIND_MASKED_STAMP,
    FLAG_CANVAS_SPACE,
    FLAG_SOFT_CONTRAST,
    prim,
} from './selection_helpers';
import BrushOptions from '../ui/BrushOptions.svelte';
import BrushBuilderPanel from '../ui/BrushBuilderPanel.svelte';
import {
    onCloneStrokeStart,
    onCloneStrokeMove,
    onCloneStrokeEnd,
    clearCloneSourceCursor,
} from './clone_source_cursor';
import { isToolHoverSuppressed } from './modifier_cursor';

/** Brush-tool session state: an app-global artist preference (not per-document),
 *  so it stays module-level even though the brush *tool* is per-instance.
 *  Persists across strokes within the session; resets on reload. The engine-side
 *  blend-mode mirror is pushed by `onActivate` / `onDeactivate` and by the
 *  toggleEraseMode action. */
class BrushSession {
    /** When true, strokes use destination-out (erase) instead of source-over. */
    eraseMode = $state(false);
}
export const brushSession = new BrushSession();

/** Soft-contrast strength for big brushes. Tuned by eye. */
const BASE_STRENGTH = 0.22;
/** Strength at or below the "small" threshold: compensates for the
 *  stamp covering fewer screen pixels by amping contrast. */
const MAX_STRENGTH = 0.65;
/** Half-extent in *on-screen* pixels where MAX_STRENGTH applies. */
const SMALL_ON_SCREEN = 6;
/** Half-extent in *on-screen* pixels at/above which BASE_STRENGTH applies. */
const LARGE_ON_SCREEN = 40;

interface BrushCursorPreviewInfo {
    halfExtent: [number, number];
}

/** Scale strength with on-screen stamp size: tiny stamps get more contrast
 *  so they remain readable; big stamps stay subtle. Smooth ramp. `zoom` is the
 *  owning instance's view zoom (on-screen size = canvas size × zoom). */
function previewStrength(halfExtent: [number, number], zoom: number): number {
    const minHE = Math.min(halfExtent[0], halfExtent[1]) * zoom;
    const t = Math.max(0, Math.min(1,
        (minHE - SMALL_ON_SCREEN) / (LARGE_ON_SCREEN - SMALL_ON_SCREEN)));
    const smooth = t * t * (3 - 2 * t);  // smoothstep
    return MAX_STRENGTH + (BASE_STRENGTH - MAX_STRENGTH) * smooth;
}

/** Pen pose passed to `refresh_brush_cursor_preview`: drives any pressure /
 *  tilt / twist dynamics wired into the brush graph. Components are in
 *  the normalised ranges WASM expects (pressure 0-1, tilt ±1, twist 0-1). */
export interface PenPose {
    pressure: number;
    tiltX: number;
    tiltY: number;
    twist: number;
    tangentialPressure: number;
}

/** Pose for the on-canvas cursor preview. Pressure is pinned to full so the
 *  circle shows the brush's reach; a hovering pen reports 0, and the preview
 *  isn't a live dab. Tilt and twist still track the live event. The resize
 *  scrub uses the same pose, keeping cursor and stroke in lockstep. */
export function cursorPose(e: PointerEvent): PenPose {
    return {
        pressure: 1.0,
        tiltX: (e.tiltX ?? 0) / 90,
        tiltY: (e.tiltY ?? 0) / 90,
        twist: (e.twist ?? 0) / 360,
        tangentialPressure: (e as any).tangentialPressure ?? 0,
    };
}

export const MIN_SIZE = 1;
export const MAX_SIZE = 500;
export const SIZE_STEP = 4;
export const INITIAL_SIZE = 24;
export const INITIAL_OPACITY = 1.0;

/** Build a brush_stroke params object from a PointerEvent + the active foreground. */
export function brushStrokeParams(e: PointerEvent, cx: number, cy: number, c: Color) {
    return {
        x: cx,
        y: cy,
        pressure: effectivePressure(e),
        x_tilt: (e.tiltX ?? 0) / 90, // normalize -90..90 → -1..1
        y_tilt: (e.tiltY ?? 0) / 90,
        rotation: (e.twist ?? 0) / 360, // normalize 0..359 → 0..1
        tangential_pressure: (e as any).tangentialPressure ?? 0,
        time_ms: e.timeStamp,
        cr: c.r / 255,
        cg: c.g / 255,
        cb: c.b / 255,
        ca: c.a / 255,
    };
}

// --- Gesture interpreter ---

class BrushTool extends ToolBase {
    /** Last hover pose+position pushed to the overlay. Cached so non-event
     *  callers (the `[` / `]` size hotkeys) can re-push at the same spot after
     *  mutating the graph. Cleared on stroke start, pointer-leave, and tool
     *  deactivate, so it only exists while a hover preview is actually visible. */
    private lastHover: { cx: number; cy: number; pose: PenPose } | null = null;

    /** Monotonic hover generation, bumped every time the overlay is invalidated
     *  (stroke start, pointer leave, tool deactivate). `pushHoverOverlay` awaits
     *  the preview refresh before drawing, so a hover in flight when a stroke
     *  begins could otherwise land its `set_overlay` *after* pointerdown's
     *  `clear_overlay`, freezing a ghost dab on-canvas for the whole stroke.
     *  Capturing the generation before the await and re-checking after lets an
     *  invalidated hover bail instead of overtaking the clear.
     *
     *  A finer-grained sibling of `tool_session.ts`: that primitive invalidates
     *  on session boundaries (tool switch, layer change); `hoverGen` also
     *  invalidates on *stroke start* within the same session, a boundary a tool
     *  session doesn't draw, so it stays. */
    private hoverGen = 0;

    /** Refresh the on-canvas brush cursor preview at `(cx, cy)` using the given
     *  pose. Also reachable by non-brush callers (the shift+drag size scrub, the
     *  `[` / `]` hotkey refresh) via {@link focusedBrushTool}. Async: it awaits
     *  the preview refresh before drawing. */
    async pushHoverOverlay(pose: PenPose, cx: number, cy: number): Promise<void> {
        const engine = this.engine;
        if (!engine) return;
        // While a modifier cursor is engaged (picker dropper, clone crosshair),
        // no hover entry path may render a dab or write the cursor slot; gating
        // here covers them all at the one choke point. This gate alone is not
        // airtight: an engagement landing *during* the await below slips past
        // it, and is caught instead by the `hoverGen` recheck (engaging runs
        // `suspendHover` → `clearHover()` → `hoverGen++`). Suppression safety is
        // the gate and the gen counter jointly: neither is redundant.
        // `restoreHover` fires only after the last engager disengages, so it
        // passes.
        if (isToolHoverSuppressed()) return;
        const gen = this.hoverGen;
        const info = (await engine.api.refreshBrushCursorPreview({
            x: cx,
            y: cy,
            pressure: pose.pressure,
            tilt_x: pose.tiltX,
            tilt_y: pose.tiltY,
            rotation: pose.twist,
            tangential_pressure: pose.tangentialPressure,
        })) as BrushCursorPreviewInfo | null;
        // A stroke / leave / deactivate fired during the await; that path
        // already cleared the overlay, so drawing now would resurrect a frozen
        // ghost dab.
        if (gen !== this.hoverGen) return;
        if (!info) {
            engine.api.clearOverlay();
            this.inst.toolCursor = null;
            this.lastHover = null;
            return;
        }
        this.inst.toolCursor = 'none';
        engine.api.setOverlay({
            primitives: [
                prim(
                    KIND_MASKED_STAMP,
                    FLAG_CANVAS_SPACE | FLAG_SOFT_CONTRAST,
                    [cx, cy],
                    info.halfExtent,
                    { modeParam: previewStrength(info.halfExtent, this.inst.zoom) },
                ),
            ],
        });
        this.lastHover = { cx, cy, pose };
    }

    /** Re-push the hover overlay at the last known hover position. No-op if the
     *  pointer isn't currently hovering the canvas. Used by hotkey-driven
     *  brush-param changes so the on-canvas preview reflects the new value
     *  without requiring pointer motion. */
    refreshHoverOverlay(): void {
        if (!this.lastHover) return;
        void runHook(this.pushHoverOverlay(this.lastHover.pose, this.lastHover.cx, this.lastHover.cy));
    }

    /** Drop the cached hover and invalidate any in-flight async push. */
    private clearHover(): void {
        this.lastHover = null;
        this.hoverGen++;
    }

    async onActivate(): Promise<void> {
        const engine = this.engine;
        if (!engine) return;
        // Initialize brush graph state from WASM on first activation.
        if (!brushGraph.graph && this.inst.engine) {
            brushGraph.init();
        }
        // Sync session erase-mode flag to the engine. Other tools that don't
        // paint never read brush_blend_mode; brush tools that do (color_output)
        // will pick this up on the next stroke.
        engine.api.setBrushBlendMode({ mode: brushSession.eraseMode ? 1 : 0 });
        // Hide the native cursor only if a preview is available; otherwise fall
        // back to the default cursor so the artist has *something* to see.
        const info = await engine.api.getBrushCursorPreviewInfo();
        // Re-check after the await: a modifier cursor may have engaged in the
        // meantime (hotkeying into the brush with the chord already held) and
        // writing now would stomp its cursor.
        if (isToolHoverSuppressed()) return;
        this.inst.toolCursor = info ? 'none' : null;
    }

    onDeactivate(): void {
        // Leaving the brush tool drops the builder's fullscreen mode so the
        // pinned bottom-area overlay can't outlive the panel that owns it.
        brushGraph.fullscreen = false;
        this.engine?.api.clearOverlay();
        // Reset engine blend mode so a future paint-capable tool (or a direct
        // WASM call) doesn't inherit our erase state.
        this.engine?.api.setBrushBlendMode({ mode: 0 });
        this.inst.toolCursor = null;
        this.clearHover();
        // Drop the on-canvas clone source marker with the tool that owns it
        // (the engine anchor persists as session state).
        clearCloneSourceCursor();
    }

    onPointerDown(e: PointerEvent, cx: number, cy: number): void {
        const engine = this.engine;
        const layerId = this.inst.activeLayerId;
        if (!layerId || !engine) return;

        // Clear the hover overlay while painting: the stamp renders onto the
        // canvas directly; a ghost at the cursor would just clutter.
        engine.api.clearOverlay();
        engine.api.clearBrushCursorPreviewPose();
        this.clearHover();
        this.inst.toolCursor = 'none';
        const params = brushStrokeParams(e, cx, cy, this.inst.consumeForeground());
        beginPaintStroke(engine, layerId);
        engine.api.strokeTo({ op: { op: 'brush_stroke', ...params } });
        // Capture the clone dest anchor so the source marker tracks the cursor
        // in aligned mode (no-op for non-clone brushes).
        onCloneStrokeStart(cx, cy);
        const dims = currentCanvasDimensions();
        if (dims) strokeRecorder.beginStroke(dims[0], dims[1], params);
    }

    onPointerMove(e: PointerEvent, cx: number, cy: number): void {
        const engine = this.engine;
        if (!engine) return;
        if (e.buttons & 1) {
            const params = brushStrokeParams(e, cx, cy, this.inst.consumeForeground());
            engine.api.strokeTo({ op: { op: 'brush_stroke', ...params } });
            strokeRecorder.addEvent(params);
            onCloneStrokeMove(cx, cy);
            return;
        }
        // Hover: re-render the preview with live pen data + draw it.
        void runHook(this.pushHoverOverlay(cursorPose(e), cx, cy));
    }

    onPointerUp(): void {
        this.engine?.api.endStroke();
        strokeRecorder.endStroke();
        onCloneStrokeEnd();
    }

    onPointerLeave(): void {
        // Pointer left the canvas: drop the hover ghost so it doesn't linger at
        // the last-seen edge position.
        this.suspendHover();
    }

    /** Tear down the hover preview: clear the on-canvas ghost, drop the engine's
     *  cached preview pose, and invalidate any in-flight async push. Shared by
     *  pointer-leave and a modifier cursor engaging. */
    suspendHover(): void {
        this.engine?.api.clearOverlay();
        this.engine?.api.clearBrushCursorPreviewPose();
        this.clearHover();
    }

    restoreHover(cx: number, cy: number): void {
        // Re-establish the dab preview after an interruption (e.g. the
        // modifier-held color picker releasing). We don't have a live
        // PointerEvent, so synthesise a mouse-like pose: full pressure, no
        // tilt/twist. Real pen poses re-assert on the next genuine pointermove.
        void runHook(
            this.pushHoverOverlay(
                { pressure: 1.0, tiltX: 0, tiltY: 0, twist: 0, tangentialPressure: 0 },
                cx, cy,
            ),
        );
    }
}

/** The focused instance's brush tool, if the brush tool is registered. Used by
 *  hover-driven callers outside the pointer pipeline (brush-param hotkeys /
 *  drag scrub) to reach the live per-instance brush without importing `app`. */
export function focusedBrushTool(): BrushTool | null {
    const inst = getActiveInstance();
    if (!inst) return null;
    const t = inst.tool('brush');
    return t instanceof BrushTool ? t : null;
}

export const brushTool: ToolDescriptor = {
    id: 'brush',
    /** Icon swaps to the eraser glyph while `brushSession.eraseMode` is on,
     *  giving the toolbar button a visible mode indicator. Reactive because
     *  Svelte's template re-reads the getter when `brushSession.eraseMode`
     *  ($state) changes. */
    get icon() {
        return brushSession.eraseMode
            ? 'fa6-solid:eraser'
            : 'fa6-solid:paintbrush';
    },
    group: 'paint',
    optionsComponent: BrushOptions,
    panelComponent: BrushBuilderPanel,
    create: (inst: DarklyInstance) => new BrushTool(inst),
};
