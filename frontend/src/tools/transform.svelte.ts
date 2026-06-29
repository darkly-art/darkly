/**
 * Transform tool — thin orchestration over the generic transform gizmo.
 *
 * The tool's only job is to decide *which consumer binding* drives the gizmo
 * for the current selection, then forward pointer/keyboard events. The gizmo
 * (`transform_gizmo.ts`) and the mode strategies know nothing about voids or
 * floating; this file is where that routing lives.
 *
 * Routing:
 *   - vector layer  → per-object: hit-test the click; on a hit attach that
 *                     object's transform binding. A different *axis* than the
 *                     whole-layer capability below, so it isn't a
 *                     `TransformCapability` arm — vector knowledge stays here.
 *   - `live`        → a void's persistent transform property (camera, …).
 *   - `destructive` → floating extract/commit (raster paste & move).
 *   - `none`        → inert: no gizmo, no cursor change.
 *
 * The `live` / `destructive` / `none` split comes from the backend's
 * `layer_transform_capability`.
 *
 * Two tools share this implementation behind a toolbar cluster: `transform`
 * (free / affine) and `transform_perspective` (enters perspective). They are
 * one tool with two entry modes — only `entryMode` differs — so there's one
 * gizmo, one set of handlers, one mode registry. Mid-session the right-click
 * menu (`TransformModeMenu`) switches modes freely.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { TransformGizmo } from './transform_gizmo';
import {
    floatingTransformBinding,
    voidTransformBinding,
    vectorObjectTransformBinding,
} from './transform_bindings';

let gizmo: TransformGizmo | null = null;

/** Mode the active tool variant enters with — set by whichever cluster member
 *  is activated. `0` (free) is the document default and is never force-applied
 *  (so re-entering an already-perspective object with the free tool doesn't
 *  silently downgrade it — that's an explicit right-click-menu action). A
 *  non-zero `entryMode` is seeded on attach so picking "Perspective" starts in
 *  perspective. */
let entryMode = 0;

/** Modes the active gizmo offers (for the right-click menu). Empty when no
 *  gizmo is up. */
export function transformModes(): { tag: number; label: string }[] {
    return gizmo?.availableModes() ?? [];
}

/** Wire tag of the gizmo's current mode, or `null` when inactive. */
export function transformActiveMode(): number | null {
    return gizmo?.active ? gizmo.modeTag : null;
}

/** Switch the active gizmo to `tag` (menu selection). */
export function setTransformMode(tag: number): void {
    gizmo?.setMode(tag);
}

/** Apply the active variant's entry mode after a fresh attach. See `entryMode`
 *  — mode 0 is the adopted default, so only a non-zero entry mode is seeded. */
function applyEntryMode(): void {
    if (entryMode !== 0) gizmo?.setMode(entryMode);
}

/** Resolve the selected layer's capability and attach the matching binding. */
async function activate(): Promise<void> {
    if (!gizmo || !app.engine || app.activeLayerId == null) return;
    const { value: cap } = await app.engine.send<{ value: string }>('layer_transform_capability', {
        id: app.activeLayerId,
    });
    if (cap === 'live') {
        if (await gizmo.attach(voidTransformBinding(app.activeLayerId))) applyEntryMode();
    } else if (cap === 'destructive') {
        // Floating extract may resolve asynchronously (content-bounds readback);
        // if it isn't ready this frame, `onFrame` picks it up once it is.
        if (!(await app.engine.send<{ value: boolean }>('has_floating')).value) {
            await app.engine.send('begin_transform', { id: app.activeLayerId });
        }
        if (await gizmo.attach(floatingTransformBinding())) applyEntryMode();
    }
    // 'none' → leave the gizmo inactive (inert no-op).
    app.requestFrame();
}

/** Build a transform tool variant. `entry` is the mode it engages with. */
function createTransformTool(opts: {
    id: string;
    icon: string;
    hotkeyAction: string;
    entry: number;
}): Tool {
    return {
        id: opts.id,
        icon: opts.icon,
        group: 'transform',
        cluster: 'transform',
        hotkeyAction: opts.hotkeyAction,

        onActivate(ctx: ToolContext) {
            gizmo = new TransformGizmo(ctx.canvasEl);
            entryMode = opts.entry;
            void activate();
        },

        onDeactivate() {
            // Finalize whatever's in flight (floating bakes; a live void is a no-op).
            gizmo?.commit();
            gizmo = null;
            app.transformModeMenu = null;
        },

        claimsPointer() {
            // Once a gizmo is up, the canvas belongs to the tool — handle drags,
            // body translate, and outside-bbox rotate are all transform gestures.
            // Claiming prevents global drag chords from intercepting our gestures.
            return gizmo?.active ?? false;
        },

        async onPointerDown(_ctx, e, cx, cy) {
            if (!gizmo) return;
            // Right-click inside the active object opens the mode-switch menu
            // (Free transform / Perspective / …). Always swallow button 2 so it
            // never starts a drag (the browser context menu is suppressed
            // app-wide in CanvasView).
            if (e.button === 2) {
                if (gizmo.active && gizmo.isInside(cx, cy)) {
                    app.transformModeMenu = { x: e.clientX, y: e.clientY };
                }
                return;
            }
            // First click (or a click after Enter/Escape) re-engages the gizmo.
            if (!gizmo.active) {
                // A click landing on a vector object attaches a per-object gizmo.
                // `hit_test_vector_object` returns -1 for a miss or a non-vector
                // layer, so this gate doubles as the "is it a vector layer" check —
                // no separate kind query, no `TransformCapability::Vector` arm.
                if (app.engine && app.activeLayerId != null) {
                    const hit = await app.engine.send<{ object: number }>('hit_test_vector_object', {
                        id: app.activeLayerId,
                        x: cx,
                        y: cy,
                    });
                    if (hit && hit.object >= 0) {
                        await gizmo.attach(vectorObjectTransformBinding(app.activeLayerId, hit.object));
                        app.requestFrame();
                    }
                }
                // Not a vector hit → fall back to void / floating routing.
                if (!gizmo.active) await activate();
            }
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
            // lands (begin_transform may defer a frame). Never auto-attaches
            // voids — those attach explicitly via activate(), so Enter/Escape
            // can end the session without it immediately reappearing.
            if (!gizmo.active && app.engine) {
                if ((await app.engine.send<{ value: boolean }>('has_floating')).value) {
                    if (await gizmo.attach(floatingTransformBinding())) applyEntryMode();
                }
            }
            await gizmo.frame();
        },

        async dismissOverlay() {
            if (!gizmo) return;
            if (app.engine && (await app.engine.send<{ value: boolean }>('has_floating')).value) {
                // Activating the floating's own target layer (e.g.
                // paste-as-floating creates a new layer and selects it) is part
                // of the floating workflow, not a user-switched-away signal.
                const { id } = await app.engine.send<{ id: number }>('floating_target_layer');
                if (id >= 0 && id === app.activeLayerId) {
                    return;
                }
            }
            // The awaits above can outlive the gizmo: a tool switch or layer
            // change may run onDeactivate (which already commits) and null it out
            // before we resume. Re-check so we neither dereference null nor
            // double-commit.
            gizmo?.commit();
        },
    };
}

/** Free (affine) transform — pan / scale / rotate. The cluster default. */
export const transformTool: Tool = createTransformTool({
    id: 'transform',
    icon: 'fa6-solid:up-down-left-right',
    hotkeyAction: 'transformTool',
    entry: 0,
});

/** Perspective transform — enters the four-corner homography mode directly. */
export const transformPerspectiveTool: Tool = createTransformTool({
    id: 'transform_perspective',
    icon: 'tabler:perspective',
    hotkeyAction: 'transformPerspectiveTool',
    entry: 1,
});
