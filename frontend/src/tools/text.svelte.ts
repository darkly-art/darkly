import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import { flushTextContent } from './text_editor';
import { TextBoxGizmo } from './text_box_gizmo';
import type { Tool, ToolContext } from './registry';

/** Session state for the text tool. The tool chooses what the properties panel
 *  edits and drives the on-canvas box; the text controls live in
 *  `TextProperties.svelte`.
 *
 *  - The style fields are creation *defaults*: written by panel style edits,
 *    read when the panel creates the next text object.
 *  - `focusObject` asks the panel to focus a specific object's editor next
 *    render (set when a text-tool click hits an existing object, or on create).
 *  - `editing` is the object whose box gizmo is shown on the canvas — the one
 *    just clicked or created. The box follows the active layer (see the tool's
 *    `onFrame`).
 *  - `placement` is a click/drag on empty canvas handed to the panel, which
 *    creates the text object there immediately (seeded "text", selected). `box`
 *    is set when it was drag-created (area text). */
class TextSession {
    /** Font size in canvas pixels. */
    size = $state(48);
    /** Family name resolved against the engine font collection. */
    fontFamily = $state('Noto Sans');
    /** start | center | end | justified */
    align = $state('start');
    italic = $state(false);
    /** CSS weight, 100–900. */
    weight = $state(400);
    /** Object the panel should focus next render, or null. */
    focusObject = $state<number | null>(null);
    /** Object whose box gizmo is drawn on the canvas, or null. */
    editing = $state<{ layerId: number; objectId: number } | null>(null);
    /** Where the panel should create the next text object (consumed on create),
     *  or null when there's no pending placement. */
    placement = $state<{
        x: number;
        y: number;
        anchorLayerId: number | null;
        box?: [number, number] | null;
    } | null>(null);
}

export const textSession = new TextSession();

/** Min canvas-px extent on each axis for a drag to count as a box (vs a click). */
const DRAG_THRESHOLD = 4;

/** The box gizmo for the object currently being edited, owned by the active
 *  tool instance (created on activate, torn down on deactivate). */
let gizmo: TextBoxGizmo | null = null;
let canvasEl: HTMLCanvasElement | null = null;

/** In-flight drag-to-create-a-box: start + current canvas position. */
let creating: { sx: number; sy: number; cx: number; cy: number; anchorLayerId: number | null } | null =
    null;

/** Draw the rubber-band rectangle for an in-flight box creation. */
function drawCreateBand(): void {
    if (!app.engine || !creating || !canvasEl) return;
    const { sx, sy, cx, cy } = creating;
    const o = new OverlayBuilder(canvasEl);
    const c: [number, number][] = [[sx, sy], [cx, sy], [cx, cy], [sx, cy]];
    for (let i = 0; i < 4; i++) o.line(c[i], c[(i + 1) % 4], { color: '#4af', thickness: 1, dash: 4 });
    o.push(app.engine);
}

function clearCreateBand(): void {
    app.engine?.post('clear_overlay');
    app.requestFrame();
}

export const textTool: Tool = {
    id: 'text',
    icon: 'at-icons:text',
    group: 'paint',
    hotkeyAction: 'textTool',

    async onActivate(ctx: ToolContext) {
        canvasEl = ctx.canvasEl;
        gizmo = new TextBoxGizmo(ctx.canvasEl);
        // Pick up the persisted default size if the user configured one.
        const cfgSize = config.get('tools.textSize');
        if (typeof cfgSize === 'number') textSession.size = cfgSize;
        // If a text layer is already selected, show its box immediately (the
        // focused object, else the topmost) so editing existing text is direct.
        const layerId = app.activeLayerId;
        if (app.engine && layerId !== null) {
            const res = await app.engine.send<{ objects: { object: number }[] }>('text_objects', {
                id: layerId,
            });
            const objs = res?.objects ?? [];
            if (objs.length) {
                const focus = textSession.focusObject;
                const pick =
                    focus !== null && objs.some((o) => o.object === focus)
                        ? focus
                        : objs[objs.length - 1].object;
                textSession.editing = { layerId, objectId: pick };
            }
        }
    },

    onDeactivate() {
        // Flush any coalesced keystroke before leaving so the last character
        // isn't dropped, then drop transient state (a never-typed placement
        // discards nothing — no layer was created).
        flushTextContent();
        textSession.placement = null;
        textSession.editing = null;
        creating = null;
        gizmo?.detach();
        gizmo = null;
        canvasEl = null;
    },

    // The text tool owns the canvas while active: it drives click-to-place,
    // drag-to-box, and handle-resize gestures. Nav (space/touch) still gets
    // first chance in CanvasView before this claim, so panning is unaffected.
    claimsPointer() {
        return true;
    },

    async onPointerDown(_ctx, e, cx, cy) {
        if (e.button !== 0) return;
        const engine = app.engine;

        // Grab a resize handle of the current box, if one is under the pointer.
        if (gizmo?.active && gizmo.pointerDown(cx, cy)) return;

        const layerId = app.activeLayerId ?? null;

        // A click on an existing text object focuses its editor and shows its
        // box. `hit_test_vector_object` covers the whole box (its bbox), so a
        // click anywhere inside it re-targets the object.
        if (engine && layerId !== null) {
            const hit = await engine.send<{ object: number }>('hit_test_vector_object', {
                id: layerId,
                x: cx,
                y: cy,
            });
            if (hit && hit.object >= 0) {
                app.selectLayer(layerId);
                textSession.focusObject = hit.object;
                textSession.editing = { layerId, objectId: hit.object };
                textSession.placement = null;
                await gizmo?.attach(layerId, hit.object);
                return;
            }
        }

        // Miss → begin a create interaction. A click commits point text; a drag
        // commits a box (decided on pointer-up). Clear any active edit/box.
        textSession.editing = null;
        gizmo?.detach();
        creating = { sx: cx, sy: cy, cx, cy, anchorLayerId: layerId };
    },

    onPointerMove(_ctx, _e, cx, cy) {
        if (gizmo?.dragging) {
            gizmo.pointerMove(cx, cy);
            return;
        }
        if (creating) {
            creating.cx = cx;
            creating.cy = cy;
            drawCreateBand();
            return;
        }
        // Hover: reflect the resize cursor over a handle.
        gizmo?.pointerMove(cx, cy);
    },

    onPointerUp() {
        if (gizmo?.dragging) {
            gizmo.pointerUp();
            return;
        }
        if (!creating) return;
        const c = creating;
        creating = null;
        clearCreateBand();
        const w = Math.abs(c.cx - c.sx);
        const h = Math.abs(c.cy - c.sy);
        // Hand the placement to the panel, which creates the text object there.
        // A drag past the threshold makes it an area-text box; a click makes it
        // point text.
        textSession.placement =
            w > DRAG_THRESHOLD && h > DRAG_THRESHOLD
                ? {
                      x: Math.min(c.sx, c.cx),
                      y: Math.min(c.sy, c.cy),
                      anchorLayerId: c.anchorLayerId,
                      box: [w, h],
                  }
                : { x: c.sx, y: c.sy, anchorLayerId: c.anchorLayerId, box: null };
    },

    onKeyDown(e) {
        if (e.key !== 'Escape') return false;
        if (creating) {
            creating = null;
            clearCreateBand();
            return true;
        }
        if (textSession.placement) {
            textSession.placement = null;
            return true;
        }
        if (textSession.editing) {
            textSession.editing = null;
            gizmo?.detach();
            return true;
        }
        return false;
    },

    async onFrame() {
        if (!gizmo) return;
        // A rubber-band create owns the overlay; don't fight it.
        if (creating) return;
        const want = textSession.editing;
        // The box follows the active layer: switching to another layer clears it.
        if (want && want.layerId !== app.activeLayerId) {
            textSession.editing = null;
            gizmo.detach();
            return;
        }
        if (want) {
            if (gizmo.isTarget(want.layerId, want.objectId)) await gizmo.frame();
            else await gizmo.attach(want.layerId, want.objectId);
        } else if (gizmo.active) {
            gizmo.detach();
        }
    },

    dismissOverlay() {
        // Unhandled keypress on the canvas: flush the coalesced keystroke and
        // drop a pending placement. The box gizmo is left to `onFrame` (it
        // detaches on a layer switch), so a freshly-created edit isn't wiped by
        // the same-click layer change that selects its new layer.
        flushTextContent();
        textSession.placement = null;
    },
};
