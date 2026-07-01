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
 * - `vectorObjectTransformBinding` — a single vector object's live transform.
 *
 * The last two are the same shape — a live, persistent, coalesced transform
 * property read by one kind and updated by another — so both are thin wrappers
 * over `liveTransformBinding`.
 */
import { toolEngine } from './tool_session';
import { affineToMat3, mat3ToAffine, type Mat3 } from './transform_projective';
import type { RequestKind } from '../engine/protocol';
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

/** Binding over the floating (paste / raster-extract) session. The transform
 *  is committed once (baked into the target raster), not composited live, so
 *  every mode is on the table. */
export function floatingTransformBinding(): TransformBinding {
    return {
        live: false,
        async read() {
            const raw = await toolEngine()?.send<
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
            toolEngine()?.post('update_floating_matrix', {
                mode_tag: modeTag,
                payload: wirePayload(matrix, modeTag),
            });
        },
        commit() {
            toolEngine()?.post('commit_floating');
        },
        cancel() {
            toolEngine()?.post('cancel_floating');
        },
    };
}

/**
 * Binding over any "live, persistent, coalesced transform property" consumer:
 * read its bbox + matrix via `readKind`, push live updates and the cancel-time
 * revert via `updateKind`. `params` (e.g. `{ id }`, `{ id, object }`) is the
 * consumer's addressing, spread into every request.
 *
 * The transform composites every frame (`live: true`), and the homography is
 * shared with the floating path, so consumers that opt in support perspective
 * like everything else.
 *
 * Commit is a no-op — edits are already live on the document and coalesced into
 * the undo stack. Cancel re-posts the transform captured on first read,
 * including its *mode*, so cancelling a consumer that was already perspective
 * restores perspective rather than a downgraded affine.
 */
export function liveTransformBinding(
    readKind: RequestKind,
    updateKind: RequestKind,
    params: Record<string, unknown>,
): TransformBinding {
    let original: { matrix: Mat3; mode: number } | null = null;
    return {
        live: true,
        async read() {
            const raw = await toolEngine()?.send<
                { ox: number; oy: number; w: number; h: number; mode: number; matrix: number[] } | null
            >(readKind, params);
            if (!raw) return null;
            const matrix = liftMatrix(raw.mode, raw.matrix);
            if (original === null) original = { matrix: [...matrix] as Mat3, mode: raw.mode };
            return {
                origin: [raw.ox, raw.oy] as [number, number],
                w: raw.w,
                h: raw.h,
                mode: raw.mode,
                matrix,
            };
        },
        update(matrix: Mat3, modeTag: number) {
            toolEngine()?.post(updateKind, {
                ...params,
                mode_tag: modeTag,
                payload: wirePayload(matrix, modeTag),
            });
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) {
                toolEngine()?.post(updateKind, {
                    ...params,
                    mode_tag: original.mode,
                    payload: wirePayload(original.matrix, original.mode),
                });
            }
        },
    };
}

/** Binding over a void layer's live transform property. */
export function voidTransformBinding(layerId: number): TransformBinding {
    return liveTransformBinding('void_transform_info', 'update_void_transform', { id: layerId });
}

/** Binding over a single vector object's live transform. */
export function vectorObjectTransformBinding(layerId: number, objectId: number): TransformBinding {
    return liveTransformBinding('vector_object_info', 'update_vector_object_transform', {
        id: layerId,
        object: objectId,
    });
}
