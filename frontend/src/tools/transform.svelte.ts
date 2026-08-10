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
 * one class with two entry modes — only `entryMode` differs — so there's one
 * gizmo, one set of handlers, one mode registry. Mid-session the right-click
 * menu (`TransformModeMenu`) switches modes freely.
 *
 * State (gizmo, entry mode) is per-instance: each editor tab owns its own
 * `TransformTool`, so a floating on one tab never aliases another's.
 */
import { ToolBase, type Tool, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { getActiveInstance } from '../state/app.svelte';
import { runHook, ToolSessionCancelled } from './tool_session';
import { toast } from '../state/toast.svelte';
import { describeTransformCapabilityRejection } from './transform_errors';
import { TransformGizmo } from './transform_gizmo';
import {
    floatingTransformBinding,
    voidTransformBinding,
    vectorObjectTransformBinding,
    type SessionAccess,
} from './transform_bindings';

class TransformTool extends ToolBase {
    private gizmo: TransformGizmo | null = null;

    /** Live accessor for this instance's current session, handed to the gizmo
     *  and every binding so they follow the instance's fresh session. */
    private readonly sessionAccess: SessionAccess = () => this.engine;

    /** The mode this variant enters with. `0` (free) is the document default
     *  and is never force-applied (so re-entering an already-perspective object
     *  with the free tool doesn't silently downgrade it — that's an explicit
     *  right-click-menu action). A non-zero `entryMode` is seeded on attach so
     *  picking "Perspective" starts in perspective. */
    private readonly entryMode: number;

    constructor(inst: DarklyInstance, entryMode: number) {
        super(inst);
        this.entryMode = entryMode;
    }

    /** Modes the active gizmo offers (for the right-click menu). Empty when no
     *  gizmo is up. */
    availableModes(): { tag: number; label: string }[] {
        return this.gizmo?.availableModes() ?? [];
    }

    /** Wire tag of the gizmo's current mode, or `null` when inactive. */
    activeModeTag(): number | null {
        return this.gizmo?.active ? this.gizmo.modeTag : null;
    }

    /** Switch the active gizmo to `tag` (menu selection). */
    setMode(tag: number): void {
        this.gizmo?.setMode(tag);
    }

    /** Apply this variant's entry mode after a fresh attach. Mode 0 is the
     *  adopted default, so only a non-zero entry mode is seeded. */
    private applyEntryMode(): void {
        if (this.entryMode !== 0) this.gizmo?.setMode(this.entryMode);
    }

    /** Resolve the selected layer's capability and attach the matching binding.
     *
     *  Engine access is routed through the tool session, and `layerId` is
     *  captured *before* the first await: reaching any line past an await proves
     *  the session survived — same layer, same tool — so the captured id is
     *  still the one being transformed. A layer/tool change mid-sequence kills
     *  the session, and the next `send` rejects with `ToolSessionCancelled`
     *  (swallowed upstream), so we never attach a binding to a layer that's no
     *  longer active. */
    private async activate(): Promise<void> {
        const engine = this.engine;
        if (!this.gizmo || !engine || this.inst.activeLayerId == null) return;
        const layerId = this.inst.activeLayerId;
        const cap = await engine.api.layerTransformCapability({ id: layerId });
        if (cap === 'live') {
            if (await this.gizmo?.attach(voidTransformBinding(this.sessionAccess, layerId))) {
                this.applyEntryMode();
            }
        } else if (cap === 'destructive') {
            // Floating extract may resolve asynchronously (content-bounds
            // readback); if it isn't ready this frame, `onFrame` picks it up
            // once it is.
            if (!(await engine.api.hasFloating())) {
                try {
                    await engine.api.beginTransform({ id: layerId });
                } catch (error) {
                    if (error instanceof ToolSessionCancelled) throw error;
                    toast.show('error', describeTransformCapabilityRejection(error), { durationMs: 6000 });
                    this.inst.requestFrame();
                    return;
                }
            }
            if (await this.gizmo?.attach(floatingTransformBinding(this.sessionAccess))) {
                this.applyEntryMode();
            }
        }
        // 'none' → leave the gizmo inactive (inert no-op).
        this.inst.requestFrame();
    }

    onActivate(): void {
        const canvasEl = this.canvasEl;
        if (!canvasEl) return;
        this.gizmo = new TransformGizmo(canvasEl, this.sessionAccess);
        // Fire-and-forget: if the session dies mid-activate (a rapid re-switch
        // or layer change), activate() rejects with ToolSessionCancelled —
        // swallow it here since onActivate can't await it.
        void runHook(this.activate());
    }

    onDeactivate(): void {
        // Finalize whatever's in flight (floating bakes; a live void is a no-op).
        this.gizmo?.commit();
        this.gizmo = null;
        this.inst.transformModeMenu = null;
    }

    claimsPointer(): boolean {
        // Once a gizmo is up, the canvas belongs to the tool — handle drags,
        // body translate, and outside-bbox rotate are all transform gestures.
        // Claiming prevents global drag chords from intercepting our gestures.
        return this.gizmo?.active ?? false;
    }

    async onPointerDown(e: PointerEvent, cx: number, cy: number): Promise<void> {
        if (!this.gizmo) return;
        // Right-click inside the active object opens the mode-switch menu
        // (Free transform / Perspective / …). Always swallow button 2 so it
        // never starts a drag (the browser context menu is suppressed
        // app-wide in CanvasView).
        if (e.button === 2) {
            if (this.gizmo.active && this.gizmo.isInside(cx, cy)) {
                this.inst.transformModeMenu = { x: e.clientX, y: e.clientY };
            }
            return;
        }
        // First click (or a click after Enter/Escape) re-engages the gizmo.
        if (!this.gizmo.active) {
            // A click landing on a vector object attaches a per-object gizmo.
            // `hit_test_vector_object` returns -1 for a miss or a non-vector
            // layer, so this gate doubles as the "is it a vector layer" check —
            // no separate kind query, no `TransformCapability::Vector` arm.
            const engine = this.engine;
            const layerId = this.inst.activeLayerId;
            if (engine && layerId != null) {
                const hit = await engine.api.hitTestVectorObject({ id: layerId, x: cx, y: cy });
                if (hit && hit.object >= 0) {
                    await this.gizmo?.attach(
                        vectorObjectTransformBinding(this.sessionAccess, layerId, hit.object),
                    );
                    this.inst.requestFrame();
                }
            }
            // Not a vector hit → fall back to void / floating routing.
            if (!this.gizmo?.active) await this.activate();
        }
        if (this.gizmo?.active) this.gizmo.pointerDown(cx, cy);
    }

    onPointerMove(e: PointerEvent, cx: number, cy: number): void {
        this.gizmo?.pointerMove(cx, cy, e.shiftKey);
    }

    onPointerUp(): void {
        this.gizmo?.pointerUp();
    }

    onKeyDown(e: KeyboardEvent): boolean {
        if (!this.gizmo?.active) return false;
        if (e.key === 'Enter') {
            this.gizmo.commit();
            return true;
        }
        if (e.key === 'Escape') {
            this.gizmo.cancel();
            return true;
        }
        return false;
    }

    async onFrame(): Promise<void> {
        if (!this.gizmo) return;
        const engine = this.engine;
        // Pick up an async floating extract once its content-bounds readback
        // lands (begin_transform may defer a frame). Never auto-attaches
        // voids — those attach explicitly via activate(), so Enter/Escape
        // can end the session without it immediately reappearing.
        //
        // The engine read is session-routed: if a tool/layer change ran
        // onDeactivate (nulling `gizmo`) mid-await, the resumed request
        // rejects and unwinds here — so the `gizmo` deref below can't hit null.
        if (!this.gizmo.active && engine) {
            if (await engine.api.hasFloating()) {
                if (await this.gizmo?.attach(floatingTransformBinding(this.sessionAccess))) {
                    this.applyEntryMode();
                }
            }
        }
        // The awaits above can outlive the gizmo: a tool switch or layer change
        // may run onDeactivate and null it out before we resume.
        await this.gizmo?.frame();
    }

    async dismissOverlay(): Promise<void> {
        const gizmo = this.gizmo;
        if (!gizmo) return;
        const engine = this.engine;
        if (engine && (await engine.api.hasFloating())) {
            // Activating the floating's own target layer (e.g.
            // paste-as-floating creates a new layer and selects it) is part of
            // the floating workflow, not a user-switched-away signal.
            const id = await engine.api.floatingTargetLayer();
            if (id !== null && id === this.inst.activeLayerId) {
                return;
            }
        }
        // Reaching here past the session-routed awaits proves the session (and
        // thus `gizmo`) is still alive, so this commit is safe and single: a
        // tool/layer change mid-await would have rejected above.
        gizmo.commit();
    }
}

/** Locate the focused instance's active transform tool, if the active tool is a
 *  transform variant. Used by `TransformModeMenu` to route mode queries/edits to
 *  the tool that owns the gizmo. */
export function focusedTransformTool(): TransformTool | null {
    const inst = getActiveInstance();
    if (!inst) return null;
    const t = inst.tool(inst.activeToolId);
    return t instanceof TransformTool ? t : null;
}

/** Descriptor factory for a transform cluster variant. */
function transformDescriptor(opts: { id: string; entry: number }): ToolDescriptor {
    return {
        id: opts.id,
        group: 'transform',
        cluster: 'transform',
        create: (inst): Tool => new TransformTool(inst, opts.entry),
    };
}

/** Free (affine) transform — pan / scale / rotate. The cluster default. */
export const transformTool: ToolDescriptor = transformDescriptor({
    id: 'transform',
    entry: 0,
});

/** Perspective transform — enters the four-corner homography mode directly. */
export const transformPerspectiveTool: ToolDescriptor = transformDescriptor({
    id: 'transform_perspective',
    entry: 1,
});
