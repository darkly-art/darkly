/**
 * Transform tool — thin orchestration over the generic transform gizmo.
 *
 * The tool's only job is to decide *which consumer binding* drives the gizmo
 * for the current selection, then forward pointer/keyboard events. The gizmo
 * (`transform_gizmo.ts`) and the mode strategies know nothing about voids or
 * floating; this file is where that routing lives.
 *
 * Routing (via the backend's `layer_transform_capability`):
 *   - `live`        → a void's persistent transform property (camera, …).
 *   - `destructive` → floating extract/commit (raster paste & move).
 *   - `none`        → inert: no gizmo, no cursor change.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { TransformGizmo } from './transform_gizmo';
import { floatingTransformBinding, voidTransformBinding } from './transform_bindings';

let gizmo: TransformGizmo | null = null;

/** Resolve the selected layer's capability and attach the matching binding. */
async function activate(): Promise<void> {
    if (!gizmo || !app.engine || app.activeLayerId == null) return;
    const { value: cap } = await app.engine.send<{ value: string }>('layer_transform_capability', {
        layer_id: app.activeLayerId,
    });
    if (cap === 'live') {
        await gizmo.attach(voidTransformBinding(app.activeLayerId));
    } else if (cap === 'destructive') {
        // Floating extract may resolve asynchronously (content-bounds readback);
        // if it isn't ready this frame, `onFrame` picks it up once it is.
        if (!(await app.engine.send<{ value: boolean }>('has_floating')).value) {
            await app.engine.send('begin_transform', { id: app.activeLayerId });
        }
        await gizmo.attach(floatingTransformBinding());
    }
    // 'none' → leave the gizmo inactive (inert no-op).
    app.requestFrame();
}

export const transformTool: Tool = {
    id: 'transform',
    icon: 'fa6-solid:up-down-left-right',
    group: 'transform',
    hotkeyAction: 'transformTool',

    onActivate(ctx: ToolContext) {
        gizmo = new TransformGizmo(ctx.canvasEl);
        void activate();
    },

    onDeactivate() {
        // Finalize whatever's in flight (floating bakes; a live void is a no-op).
        gizmo?.commit();
        gizmo = null;
    },

    claimsPointer() {
        // Once a gizmo is up, the canvas belongs to the tool — handle drags,
        // body translate, and outside-bbox rotate are all transform gestures.
        // Claiming prevents global drag chords from intercepting our gestures.
        return gizmo?.active ?? false;
    },

    async onPointerDown(_ctx, _e, cx, cy) {
        if (!gizmo) return;
        // First click (or a click after Enter/Escape) re-engages the gizmo.
        if (!gizmo.active) await activate();
        if (gizmo.active) gizmo.pointerDown(cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        gizmo?.pointerMove(cx, cy, e.shiftKey);
    },

    onPointerUp() {
        gizmo?.pointerUp();
    },

    onKeyDown(e) {
        if (!gizmo?.active) return false;
        if (e.key === 'Enter') {
            gizmo.commit();
            return true;
        }
        if (e.key === 'Escape') {
            gizmo.cancel();
            return true;
        }
        return false;
    },

    async onFrame() {
        if (!gizmo) return;
        // Pick up an async floating extract once its content-bounds readback
        // lands (begin_transform may defer a frame). Never auto-attaches voids
        // — those attach explicitly via activate(), so Enter/Escape can end
        // the session without it immediately reappearing.
        if (!gizmo.active && app.engine) {
            if ((await app.engine.send<{ value: boolean }>('has_floating')).value) {
                await gizmo.attach(floatingTransformBinding());
            }
        }
        await gizmo.frame();
    },

    async dismissOverlay() {
        if (!gizmo) return;
        if (app.engine && (await app.engine.send<{ value: boolean }>('has_floating')).value) {
            // Activating the floating's own target layer (e.g. paste-as-floating
            // creates a new layer and selects it) is part of the floating
            // workflow, not a user-switched-away signal.
            const { id } = await app.engine.send<{ id: number }>('floating_target_layer');
            if (id >= 0 && id === app.activeLayerId) {
                return;
            }
        }
        gizmo.commit();
    },
};
