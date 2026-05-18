import { actions } from './registry';
import { app } from '../state/app.svelte';
import { brushGraph, exposedDragSpeed } from '../state/brush_graph.svelte';
import { pushHoverOverlay, cursorPose, refreshHoverOverlay } from '../tools/brush.svelte';

/** Map of semantic role → which exposed port adjusts it.
 *
 * Adding a new role here (and registering the matching `brush<Role><Up|Down>` /
 * `brush<Role>Adjust` actions below) is all that's required to make a new
 * brush parameter keyboard-and-drag adjustable.
 *
 * - `kind: 'mult'` applies a multiplicative step (`v * step^dir`). Use for
 *   parameters that feel logarithmic to users (size).
 * - `kind: 'add'` applies an additive step (`v + step*dir`). Use for
 *   parameters that feel linear (opacity, hardness).
 */
type RoleSpec = { portName: string; step: number; kind: 'mult' | 'add' };
const ROLE_MAP: Record<string, RoleSpec> = {
    size: { portName: 'size', step: 1.15, kind: 'mult' },
    // Future roles plug in here without changing call sites:
    // opacity:  { portName: 'flow',     step: 0.05, kind: 'add' },
    // hardness: { portName: 'softness', step: 0.05, kind: 'add' },
};

type Role = keyof typeof ROLE_MAP;

/** Find the exposed scalar port that a given role refers to, if present in
 *  the active brush. Returns null when the brush graph doesn't expose it. */
function findScalarPort(role: Role) {
    const spec = ROLE_MAP[role];
    const port = brushGraph.exposedPorts.find(p => p.portName === spec.portName);
    if (!port || port.data.kind !== 'scalar') return null;
    return { port, data: port.data, spec };
}

function clamp(v: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, v));
}

function commit(nodeId: number, portName: string, value: number) {
    brushGraph.setExposedPortValueLocal(nodeId, portName, value);
    brushGraph.setExposedPortValue(nodeId, portName, value);
}

/** Discrete step in the given direction (used by `[` / `]` hotkeys).
 *  After committing, refresh the on-canvas hover overlay so the brush
 *  cursor preview reflects the new value immediately — without this
 *  the circle stays at the old size until the user moves the pointer. */
function adjustBrushParam(role: Role, dir: 1 | -1): void {
    const found = findScalarPort(role);
    if (!found) return;
    const { port, data, spec } = found;
    const next =
        spec.kind === 'mult'
            ? data.value * Math.pow(spec.step, dir)
            : data.value + spec.step * dir;
    commit(port.nodeId, port.portName, clamp(next, data.min, data.max));
    if (app.handle) refreshHoverOverlay(app.handle);
}

/** Set an absolute value (used by drag scrubs). */
function setBrushParam(role: Role, absolute: number): void {
    const found = findScalarPort(role);
    if (!found) return;
    const { port, data } = found;
    commit(port.nodeId, port.portName, clamp(absolute, data.min, data.max));
}

/** Transient state held while a drag-scrub is in flight. Module-level
 *  because there's only ever one active drag at a time, gated by
 *  pointer capture. The cursor preview is *anchored* at the pointerdown
 *  position so only size changes visibly during the drag. */
type SizeDragState = {
    startVal: number;
    anchorX: number;
    anchorY: number;
};
let sizeDrag: SizeDragState | null = null;

export function registerBrushParamActions() {
    actions.register({
        id: 'brushSizeUp',
        displayName: 'Increase Brush Size',
        category: 'brush',
        defaultHotkey: 'BracketRight',
        handler: () => adjustBrushParam('size', +1),
    });

    actions.register({
        id: 'brushSizeDown',
        displayName: 'Decrease Brush Size',
        category: 'brush',
        defaultHotkey: 'BracketLeft',
        handler: () => adjustBrushParam('size', -1),
    });

    actions.register({
        id: 'brushSizeAdjust',
        displayName: 'Adjust Brush Size (drag)',
        category: 'brush',
        type: 'hold',
        defaultMouseClick: 'canvas:shift+drag',
        handler: (ctx) => {
            const found = findScalarPort('size');
            if (!found) {
                sizeDrag = null;
                return;
            }
            sizeDrag = {
                startVal: found.data.value,
                anchorX: typeof ctx.x === 'number' ? ctx.x : 0,
                anchorY: typeof ctx.y === 'number' ? ctx.y : 0,
            };
        },
        onMove: (_ctx, e, dx) => {
            if (!sizeDrag) return;
            const found = findScalarPort('size');
            if (!found) return;
            const speed = exposedDragSpeed(found.data.min, found.data.max);
            setBrushParam('size', sizeDrag.startVal + dx * speed);
            // Re-render the on-canvas brush cursor preview so the circle
            // grows/shrinks live during the drag. Anchored at the start
            // position; pose comes from the live event, matching the
            // normal hover preview (pressure forced to 1 by `cursorPose`
            // so both paths show the same brush extent).
            if (app.handle) {
                pushHoverOverlay(
                    app.handle,
                    cursorPose(e),
                    sizeDrag.anchorX,
                    sizeDrag.anchorY,
                );
            }
        },
        deactivate: () => {
            sizeDrag = null;
        },
    });
}
