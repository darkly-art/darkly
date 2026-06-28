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
import { affineToMat3, mat3ToAffine, type Mat3 } from './transform_projective';
import type { TransformBinding } from './transform_gizmo';

/** Shape the wire payload for a `(matrix, modeTag)` update: basic mode (tag 0)
 *  carries the 6 affine components; perspective (tag 1) the full 9-float
 *  homography. The shared Rust decoder `Transform::from_tag_payload` picks the
 *  variant by tag. */
function wirePayload(matrix: Mat3, modeTag: number): number[] {
    return modeTag === 0 ? mat3ToAffine(matrix) : Array.from(matrix);
}

/** Lift a wire `matrix` payload back to a `Mat3` by mode: 6 floats (affine,
 *  tag 0) widen to a homography; 9 floats (perspective, tag 1) are used as-is. */
function liftMatrix(mode: number, matrix: number[]): Mat3 {
    return mode === 1 ? (matrix as Mat3) : affineToMat3(matrix as [number, number, number, number, number, number]);
}

/** Binding over the floating (paste / raster-extract) session. */
export function floatingTransformBinding(): TransformBinding {
    return {
        async read() {
            const raw = await app.engine?.send<
                { ox: number; oy: number; w: number; h: number; mode: number; matrix: number[] } | null
            >('floating_info');
            if (!raw) return null;
            return {
                origin: [raw.ox, raw.oy] as [number, number],
                w: raw.w,
                h: raw.h,
                mode: raw.mode,
                matrix: liftMatrix(raw.mode, raw.matrix),
            };
        },
        update(matrix: Mat3, modeTag: number) {
            app.engine?.post('update_floating_matrix', {
                mode_tag: modeTag,
                payload: wirePayload(matrix, modeTag),
            });
        },
        commit() {
            app.engine?.post('commit_floating');
        },
        cancel() {
            app.engine?.post('cancel_floating');
        },
    };
}

/** Binding over a void layer's live transform property. Voids stay affine
 *  (always `mode_tag: 0`); perspective for voids is a documented follow-up. */
export function voidTransformBinding(layerId: number): TransformBinding {
    // Captured on first read so Escape (cancel) reverts to the transform as it
    // was when the gizmo attached. Commit is a no-op: the edits are already
    // live on the document and coalesced into the undo stack.
    let original: Mat3 | null = null;
    return {
        async read() {
            const raw = await app.engine?.send<
                { ox: number; oy: number; w: number; h: number; mode: number; matrix: number[] } | null
            >('void_transform_info', { id: layerId });
            if (!raw) return null;
            const matrix = liftMatrix(raw.mode, raw.matrix);
            if (original === null) original = [...matrix] as Mat3;
            return {
                origin: [raw.ox, raw.oy] as [number, number],
                w: raw.w,
                h: raw.h,
                mode: raw.mode,
                matrix,
            };
        },
        update(matrix: Mat3, modeTag: number) {
            app.engine?.post('update_void_transform', {
                id: layerId,
                mode_tag: modeTag,
                payload: wirePayload(matrix, modeTag),
            });
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) {
                app.engine?.post('update_void_transform', {
                    id: layerId,
                    mode_tag: 0,
                    payload: wirePayload(original, 0),
                });
            }
        },
    };
}
