/**
 * Consumer bindings for the transform gizmo. Each wires the generic gizmo to a
 * specific consumer's protocol surface — this is where knowledge of *what's
 * being transformed* lives (deliberately NOT in the gizmo).
 *
 * Reads go through the async `engine.send` request/response transport; live
 * updates and commit/cancel are fire-and-forget `engine.post`.
 *
 * - `floatingTransformBinding` — destructive raster extract/commit (paste &
 *   move). The floating session must already be begun (`begin_transform`)
 *   before `read()` returns non-null.
 * - `voidTransformBinding` — a void's live, persistent transform property.
 */
import { app } from '../state/app.svelte';
import type { Affine2D } from './transform_affine';
import type { TransformBinding } from './transform_gizmo';

/** Binding over the floating (paste / raster-extract) session. */
export function floatingTransformBinding(): TransformBinding {
    return {
        async read() {
            const raw = await app.engine?.send<
                { ox: number; oy: number; w: number; h: number; matrix: number[] } | null
            >('floating_info');
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
            app.engine?.post('update_floating_matrix', { matrix: affine });
        },
        commit() {
            app.engine?.post('commit_floating');
        },
        cancel() {
            app.engine?.post('cancel_floating');
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
            const raw = await app.engine?.send<
                { ox: number; oy: number; w: number; h: number; mode: number; matrix: number[] } | null
            >('void_transform_info', { layer_id: layerId });
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
            app.engine?.post('update_void_transform', {
                layer_id: layerId,
                mode_tag: modeTag,
                payload: Array.from(affine),
            });
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) {
                app.engine?.post('update_void_transform', {
                    layer_id: layerId,
                    mode_tag: 0,
                    payload: Array.from(original),
                });
            }
        },
    };
}
