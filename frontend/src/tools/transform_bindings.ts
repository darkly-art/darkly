/**
 * Consumer bindings for the transform gizmo. Each wires the generic gizmo to a
 * specific consumer's protocol surface — this is where knowledge of *what's
 * being transformed* lives (deliberately NOT in the gizmo).
 *
 * Reads go through the async typed request transport (`engine.api.*`); live
 * updates and commit/cancel are fire-and-forget.
 *
 * - `floatingTransformBinding` — destructive raster extract/commit (paste &
 *   move). The floating session must already be begun (`begin_transform`)
 *   before `read()` returns non-null.
 * - `voidTransformBinding` — a void's live, persistent transform property.
 */
import { app } from '../state/app.svelte';
import type { Affine2D } from './transform_affine';
import type { TransformBinding } from './transform_gizmo';
import type { Transform } from '../engine/protocol_gen';

/** The Rust `Transform` enum's wire form (adjacently tagged `{ mode, data }`).
 *  Mirrors `Transform::mode_tag`: tag 0 is the only mode today (`Basic`, a raw
 *  affine). A new interaction mode adds a case here and a variant there. */
function transformWire(modeTag: number, affine: Affine2D): Transform {
    if (modeTag !== 0) throw new Error(`unknown transform mode tag: ${modeTag}`);
    return { mode: 'Basic', data: Array.from(affine) as Transform['data'] };
}

/** Binding over the floating (paste / raster-extract) session. */
export function floatingTransformBinding(): TransformBinding {
    return {
        async read() {
            const raw = await app.engine?.api.floatingInfo();
            if (!raw) return null;
            return {
                origin: [raw.ox, raw.oy] as [number, number],
                w: raw.w,
                h: raw.h,
                mode: 0, // floating is always basic affine
                affine: raw.matrix as Affine2D,
            };
        },
        update(affine: Affine2D) {
            app.engine?.api.updateFloatingMatrix({ matrix: affine });
        },
        commit() {
            app.engine?.api.commitFloating();
        },
        cancel() {
            app.engine?.api.cancelFloating();
        },
    };
}

/** Binding over a void layer's live transform property. */
export function voidTransformBinding(layerId: number): TransformBinding {
    // Captured on first read so Escape (cancel) reverts to the transform as it
    // was when the gizmo attached. Commit is a no-op: the edits are already
    // live on the document and coalesced into the undo stack.
    let original: Affine2D | null = null;
    return {
        async read() {
            const raw = await app.engine?.api.voidTransformInfo({ id: layerId });
            if (!raw) return null;
            const affine = raw.matrix as Affine2D;
            if (original === null) original = [...affine];
            return {
                origin: [raw.ox, raw.oy] as [number, number],
                w: raw.w,
                h: raw.h,
                mode: raw.mode,
                affine,
            };
        },
        update(affine: Affine2D, modeTag: number) {
            app.engine?.api.updateVoidTransform({
                id: layerId,
                transform: transformWire(modeTag, affine),
            });
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) {
                app.engine?.api.updateVoidTransform({
                    id: layerId,
                    transform: transformWire(0, original),
                });
            }
        },
    };
}
