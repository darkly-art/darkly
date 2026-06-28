/**
 * Transform-mode registry. Maps the `mode_tag` carried over the WASM boundary
 * (matching Rust `Transform::mode_tag`) to its frontend strategy. Adding a mode
 * is a new file + one entry here; the gizmo shell stays mode-agnostic.
 */
import { basicMode } from './basic';
import { perspectiveMode } from './perspective';
import type { TransformMode } from './types';

export type { GizmoGeometry, BBoxPolygon, TransformMode, DragSession } from './types';
export { pointInPolygon } from './types';

const MODES: Record<number, TransformMode> = {
    [basicMode.tag]: basicMode,
    [perspectiveMode.tag]: perspectiveMode,
};

/** Resolve a mode by tag, falling back to basic for unknown tags. */
export function modeForTag(tag: number): TransformMode {
    return MODES[tag] ?? basicMode;
}

/** All registered modes, in tag order. The mode-switch menu enumerates these
 *  (filtered by consumer liveness); adding a mode here surfaces it everywhere. */
export function allModes(): TransformMode[] {
    return Object.values(MODES).sort((a, b) => a.tag - b.tag);
}
