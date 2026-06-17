/**
 * Consumer bindings for the transform gizmo. Each wires the generic gizmo to a
 * specific consumer's WASM surface — this is where knowledge of *what's being
 * transformed* lives (deliberately NOT in the gizmo).
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
        read() {
            const raw = app.handle?.floating_info();
            if (!raw) return null;
            return {
                origin: [raw[0], raw[1]],
                w: raw[2],
                h: raw[3],
                mode: 0, // floating is always basic affine
                affine: [raw[4], raw[5], raw[6], raw[7], raw[8], raw[9]],
            };
        },
        update(affine: Affine2D) {
            app.handle?.update_floating_matrix(new Float32Array(affine));
        },
        commit() {
            app.handle?.commit_floating();
        },
        cancel() {
            app.handle?.cancel_floating();
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
        read() {
            const raw = app.handle?.void_transform_info(layerId);
            if (!raw) return null;
            const affine: Affine2D = [raw[5], raw[6], raw[7], raw[8], raw[9], raw[10]];
            if (original === null) original = [...affine];
            return {
                origin: [raw[0], raw[1]],
                w: raw[2],
                h: raw[3],
                mode: raw[4],
                affine,
            };
        },
        update(affine: Affine2D, modeTag: number) {
            app.handle?.update_void_transform(layerId, modeTag, new Float32Array(affine));
        },
        commit() {
            // Live + undoable already; nothing to finalize.
        },
        cancel() {
            if (original) {
                app.handle?.update_void_transform(layerId, 0, new Float32Array(original));
            }
        },
    };
}
