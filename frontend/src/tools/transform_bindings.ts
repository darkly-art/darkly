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
import { affineToMat3, mat3ToAffine, type Mat3 } from './transform_projective';
import type { TransformBinding } from './transform_gizmo';
import type { Transform } from '../engine/protocol_gen';

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
            const raw = await app.engine?.api.floatingInfo();
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
            app.engine?.api.updateFloatingMatrix({ transform: transformWire(matrix, modeTag) });
        },
        commit() {
            app.engine?.api.commitFloating();
        },
        cancel() {
            app.engine?.api.cancelFloating();
        },
    };
}

/** Binding over a void layer's live transform property. The void composites
 *  its transform every frame (`live: true`), and the homography is shared with
 *  the floating path, so voids support perspective like everything else. */
export function voidTransformBinding(layerId: number): TransformBinding {
    // Captured on first read so Escape (cancel) reverts to the transform as it
    // was when the gizmo attached — including its *mode*, so cancelling a void
    // that was already perspective restores perspective, not a downgraded
    // affine. Commit is a no-op: edits are already live and coalesced into undo.
    let original: { matrix: Mat3; mode: number } | null = null;
    return {
        live: true,
        async read() {
            const raw = await app.engine?.api.voidTransformInfo({ id: layerId });
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
            app.engine?.api.updateVoidTransform({
                id: layerId,
                transform: transformWire(matrix, modeTag),
            });
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) {
                app.engine?.api.updateVoidTransform({
                    id: layerId,
                    transform: transformWire(original.matrix, original.mode),
                });
            }
        },
    };
}
