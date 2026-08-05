import { getActiveInstance, type DarklyInstance } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import { flushTextContent } from './text_editor';
import { TextBoxGizmo } from './text_box_gizmo';
import { ToolBase, type ToolDescriptor } from './registry';

/** Creation *defaults* for the text tool — an app-global user preference (not
 *  per-document), written by panel style edits and read when the panel creates
 *  the next text object. Stays module-level even though the text *tool* is
 *  per-instance (the per-document edit/placement state moved onto {@link
 *  TextTool}). */
class TextSession {
    /** Font size in canvas pixels. */
    size = $state(48);
    /** Family name resolved against the engine font collection. */
    fontFamily = $state('Noto Sans');
    /** start | center | end | justified */
    align = $state('start');
    italic = $state(false);
    /** Variable-font axis defaults (tag → value), including `wght`. Merged into
     *  by panel edits; empty means every axis takes the font's own default. */
    variations = $state<Record<string, number>>({});
    /** Extra horizontal space between letters, in canvas pixels. */
    letterSpacing = $state(0);
    /** Extra horizontal space between words, in canvas pixels. */
    wordSpacing = $state(0);
    /** Multiplier on the font's natural line height (1.0 = natural). */
    lineHeight = $state(1.2);
}

export const textSession = new TextSession();

/** The object whose box gizmo is shown, keyed by layer + object. */
export interface TextEditing {
    layerId: number;
    objectId: number;
}

/** A click/drag on empty canvas handed to the panel, which creates the text
 *  object there immediately (seeded "text", selected). `box` is set when it was
 *  drag-created (area text). */
export interface TextPlacement {
    x: number;
    y: number;
    anchorLayerId: number | null;
    box?: [number, number] | null;
}

/** Min canvas-px extent on each axis for a drag to count as a box (vs a click). */
const DRAG_THRESHOLD = 4;

/**
 * Text tool — per-instance. Chooses what the properties panel edits and drives
 * the on-canvas box; the text controls live in `TextProperties.svelte`.
 *
 * The per-document edit state lives here as reactive fields (read by the panel
 * via {@link focusedTextTool}):
 *   - `focusObject` asks the panel to focus a specific object's editor next
 *     render (set when a text-tool click hits an existing object, or on create).
 *   - `editing` is the object whose box gizmo is shown — the one just clicked or
 *     created. The box follows the active layer (see `onFrame`).
 *   - `placement` is a pending create the panel consumes.
 */
class TextTool extends ToolBase {
    /** Object the panel should focus next render, or null. */
    focusObject = $state<number | null>(null);
    /** Object whose box gizmo is drawn on the canvas, or null. */
    editing = $state<TextEditing | null>(null);
    /** Where the panel should create the next text object (consumed on create),
     *  or null when there's no pending placement. */
    placement = $state<TextPlacement | null>(null);

    /** The box gizmo for the object currently being edited, created on activate,
     *  torn down on deactivate. */
    private gizmo: TextBoxGizmo | null = null;

    /** In-flight drag-to-create-a-box: start + current canvas position. */
    private creating:
        | { sx: number; sy: number; cx: number; cy: number; anchorLayerId: number | null }
        | null = null;

    /** Draw the rubber-band rectangle for an in-flight box creation. */
    private drawCreateBand(): void {
        const engine = this.inst.engine;
        const canvasEl = this.canvasEl;
        if (!engine || !this.creating || !canvasEl) return;
        const { sx, sy, cx, cy } = this.creating;
        const o = new OverlayBuilder(canvasEl);
        const c: [number, number][] = [[sx, sy], [cx, sy], [cx, cy], [sx, cy]];
        for (let i = 0; i < 4; i++) o.line(c[i], c[(i + 1) % 4], { color: '#4af', thickness: 1, dash: 4 });
        o.push(engine);
    }

    private clearCreateBand(): void {
        this.inst.engine?.api.clearOverlay();
        this.inst.requestFrame();
    }

    async onActivate(): Promise<void> {
        const canvasEl = this.canvasEl;
        if (!canvasEl) return;
        this.gizmo = new TextBoxGizmo(canvasEl);
        // Pick up the persisted default size if the user configured one.
        const cfgSize = config.get('tools.textSize');
        if (typeof cfgSize === 'number') textSession.size = cfgSize;
        // If a text layer is already selected, show its box immediately (the
        // focused object, else the topmost) so editing existing text is direct.
        const layerId = this.inst.activeLayerId;
        if (this.inst.engine && layerId !== null) {
            const res = await this.inst.engine.api.textObjects({ id: layerId });
            const objs = res?.objects ?? [];
            if (objs.length) {
                const focus = this.focusObject;
                const pick =
                    focus !== null && objs.some((o) => o.object === focus)
                        ? focus
                        : objs[objs.length - 1].object;
                this.editing = { layerId, objectId: pick };
            }
        }
    }

    onDeactivate(): void {
        // Flush any coalesced keystroke before leaving so the last character
        // isn't dropped, then drop transient state (a never-typed placement
        // discards nothing — no layer was created).
        flushTextContent();
        this.placement = null;
        this.editing = null;
        this.creating = null;
        this.gizmo?.detach();
        this.gizmo = null;
    }

    // The text tool owns the canvas while active: it drives click-to-place,
    // drag-to-box, and handle-resize gestures. Nav (space/touch) still gets
    // first chance in CanvasView before this claim, so panning is unaffected.
    claimsPointer(): boolean {
        return true;
    }

    async onPointerDown(e: PointerEvent, cx: number, cy: number): Promise<void> {
        if (e.button !== 0) return;
        const engine = this.inst.engine;

        // Grab a resize handle of the current box, if one is under the pointer.
        if (this.gizmo?.active && this.gizmo.pointerDown(cx, cy)) return;

        const layerId = this.inst.activeLayerId ?? null;

        // A click on an existing text object focuses its editor and shows its
        // box. `hit_test_vector_object` covers the whole box (its bbox), so a
        // click anywhere inside it re-targets the object.
        if (engine && layerId !== null) {
            const hit = await engine.api.hitTestVectorObject({
                id: layerId,
                x: cx,
                y: cy,
            });
            if (hit && hit.object >= 0) {
                this.inst.selectLayer(layerId);
                this.focusObject = hit.object;
                this.editing = { layerId, objectId: hit.object };
                this.placement = null;
                await this.gizmo?.attach(layerId, hit.object);
                return;
            }
        }

        // Miss → begin a create interaction. A click commits point text; a drag
        // commits a box (decided on pointer-up). Clear any active edit/box.
        this.editing = null;
        this.gizmo?.detach();
        this.creating = { sx: cx, sy: cy, cx, cy, anchorLayerId: layerId };
    }

    onPointerMove(_e: PointerEvent, cx: number, cy: number): void {
        if (this.gizmo?.dragging) {
            this.gizmo.pointerMove(cx, cy);
            return;
        }
        if (this.creating) {
            this.creating.cx = cx;
            this.creating.cy = cy;
            this.drawCreateBand();
            return;
        }
        // Hover: reflect the resize cursor over a handle.
        this.gizmo?.pointerMove(cx, cy);
    }

    onPointerUp(): void {
        if (this.gizmo?.dragging) {
            this.gizmo.pointerUp();
            return;
        }
        if (!this.creating) return;
        const c = this.creating;
        this.creating = null;
        this.clearCreateBand();
        const w = Math.abs(c.cx - c.sx);
        const h = Math.abs(c.cy - c.sy);
        // Hand the placement to the panel, which creates the text object there.
        // A drag past the threshold makes it an area-text box; a click makes it
        // point text.
        this.placement =
            w > DRAG_THRESHOLD && h > DRAG_THRESHOLD
                ? {
                      x: Math.min(c.sx, c.cx),
                      y: Math.min(c.sy, c.cy),
                      anchorLayerId: c.anchorLayerId,
                      box: [w, h],
                  }
                : { x: c.sx, y: c.sy, anchorLayerId: c.anchorLayerId, box: null };
    }

    onKeyDown(e: KeyboardEvent): boolean {
        if (e.key !== 'Escape') return false;
        if (this.creating) {
            this.creating = null;
            this.clearCreateBand();
            return true;
        }
        if (this.placement) {
            this.placement = null;
            return true;
        }
        if (this.editing) {
            this.editing = null;
            this.gizmo?.detach();
            return true;
        }
        return false;
    }

    async onFrame(): Promise<void> {
        if (!this.gizmo) return;
        // A rubber-band create owns the overlay; don't fight it.
        if (this.creating) return;
        const want = this.editing;
        // The box follows the active layer: switching to another layer clears it.
        if (want && want.layerId !== this.inst.activeLayerId) {
            this.editing = null;
            this.gizmo.detach();
            return;
        }
        if (want) {
            if (this.gizmo.isTarget(want.layerId, want.objectId)) await this.gizmo.frame();
            else await this.gizmo.attach(want.layerId, want.objectId);
        } else if (this.gizmo.active) {
            this.gizmo.detach();
        }
    }

    dismissOverlay(): void {
        // Unhandled keypress on the canvas: flush the coalesced keystroke and
        // drop a pending placement. The box gizmo is left to `onFrame` (it
        // detaches on a layer switch), so a freshly-created edit isn't wiped by
        // the same-click layer change that selects its new layer.
        flushTextContent();
        this.placement = null;
    }
}

/** The focused instance's text tool, if the text tool is the active tool. The
 *  properties panel routes its per-document edit reads (editing / placement /
 *  focusObject) through this — reactive through the `app` proxy / instance
 *  swap. */
export function focusedTextTool(): TextTool | null {
    const inst = getActiveInstance();
    if (!inst) return null;
    const t = inst.tool('text');
    return t instanceof TextTool ? t : null;
}

export const textTool: ToolDescriptor = {
    id: 'text',
    group: 'paint',
    hotkeyAction: 'textTool',
    create: (inst: DarklyInstance) => new TextTool(inst),
};
