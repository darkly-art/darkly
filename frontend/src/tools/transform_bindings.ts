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
 * - `vectorObjectTransformBinding` — a single vector object's live transform.
 *
 * The last two are the same shape — a live, persistent, coalesced transform
 * property read by one typed call and updated by another — so both are thin
 * wrappers over `liveTransformBinding`, differing only in the typed api calls
 * they close over.
 */
import { toolEngine } from './tool_session';
import { affineToMat3, mat3ToAffine, type Mat3 } from './transform_projective';
import type { TransformBinding } from './transform_gizmo';
import type { Transform } from '../engine/protocol_gen';

/** The flat transform info the engine returns for a floating/void/vector query
 *  — `matrix` is 6 affine floats (mode 0) or 9 homography floats (mode 1). */
type TransformInfo = { ox: number; oy: number; w: number; h: number; mode: number; matrix: number[] };

/** Wrap a `(matrix, modeTag)` pair as the Rust `Transform` enum's wire form
 *  (adjacently tagged `{ mode, data }`): basic mode (tag 0) carries the 6 affine
 *  components; perspective (tag 1) the full 9-float homography. The Rust side
 *  picks the variant by tag. */
function transformWire(matrix: Mat3, modeTag: number): Transform {
    return modeTag === 1
        ? { mode: 'Perspective', data: Array.from(matrix) as [number, number, number, number, number, number, number, number, number] }
        : { mode: 'Basic', data: mat3ToAffine(matrix) as [number, number, number, number, number, number] };
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
            const raw = await toolEngine()?.api.floatingInfo();
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
            toolEngine()?.api.updateFloatingMatrix({ transform: transformWire(matrix, modeTag) });
        },
        commit() {
            toolEngine()?.api.commitFloating();
        },
        cancel() {
            toolEngine()?.api.cancelFloating();
        },
    };
}

/**
 * Binding over any "live, persistent, coalesced transform property" consumer:
 * `read` fetches its bbox + matrix, `update` pushes a live matrix and the
 * cancel-time revert. Both close over the consumer's typed api calls, so the
 * generic binding stays ignorant of which kind it's driving.
 *
 * The transform composites every frame (`live: true`), and the homography is
 * shared with the floating path, so consumers that opt in support perspective
 * like everything else.
 *
 * Commit is a no-op — edits are already live on the document and coalesced into
 * the undo stack. Cancel re-pushes the transform captured on first read,
 * including its *mode*, so cancelling a consumer that was already perspective
 * restores perspective rather than a downgraded affine.
 */
export function liveTransformBinding(
    read: () => Promise<TransformInfo | null> | undefined,
    update: (matrix: Mat3, modeTag: number) => void,
): TransformBinding {
    let original: { matrix: Mat3; mode: number } | null = null;
    return {
        live: true,
        async read() {
            const raw = await read();
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
            update(matrix, modeTag);
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) update(original.matrix, original.mode);
        },
    };
}

/** Binding over a void layer's live transform property. */
export function voidTransformBinding(layerId: number): TransformBinding {
    return liveTransformBinding(
        () => toolEngine()?.api.voidTransformInfo({ id: layerId }),
        (matrix, modeTag) =>
            toolEngine()?.api.updateVoidTransform({ id: layerId, transform: transformWire(matrix, modeTag) }),
    );
}

/** Binding over a single vector object's live transform. Vector objects carry
 *  an affine only, so the mode tag is ignored and the 6-float payload is sent
 *  raw (the engine reorders it into kurbo). */
export function vectorObjectTransformBinding(layerId: number, objectId: number): TransformBinding {
    return liveTransformBinding(
        () => toolEngine()?.api.vectorObjectInfo({ id: layerId, object: objectId }),
        (matrix) =>
            toolEngine()?.api.updateVectorObjectTransform({
                id: layerId,
                object: objectId,
                payload: mat3ToAffine(matrix),
            }),
    );
}
