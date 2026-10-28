import { actions } from './registry';
import { clamp } from '../lib/clamp';
import { app } from '../state/app.svelte';
import { brushGraph } from '../state/brush_graph.svelte';
import { canvasToScreen } from '../canvas/coordinates';
import { cursorPose, focusedBrushTool } from '../tools/brush.svelte';
import { runHook } from '../tools/tool_session';

/** Map of semantic role → which exposed port adjusts it.
 *
 * Adding a new role here (and registering the matching `brush<Role><Up|Down>` /
 * `brush<Role>Adjust` actions below) is all that's required to make a new
 * brush parameter keyboard-and-drag adjustable.
 *
 * - `kind: 'mult'` applies a multiplicative step (`v * step^dir`). Use for
 *   parameters that feel logarithmic to artists (size).
 * - `kind: 'add'` applies an additive step (`v + step*dir`). Use for
 *   parameters that feel linear (opacity, hardness).
 */
type RoleSpec = { portName: string; step: number; kind: 'mult' | 'add' };
const ROLE_MAP: Record<string, RoleSpec> = {
    // `size` is the base-size knob, owned exclusively by `pen_input` (the
    // terminals' `size` is per-touch modulation and is not exposed), so this
    // exact `portName === 'size'` match resolves unambiguously to pen_input.
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


function commit(nodeId: string, portName: string, value: number) {
    brushGraph.setExposedPortValueLocal(nodeId, portName, value);
    brushGraph.setExposedPortValue(nodeId, portName, value);
}

/** Discrete step in the given direction (used by `[` / `]` hotkeys).
 *  After committing, refresh the on-canvas hover overlay so the brush
 *  cursor preview reflects the new value immediately; without this
 *  the circle stays at the old size until the artist moves the pointer. */
function adjustBrushParam(role: Role, dir: 1 | -1): void {
    const found = findScalarPort(role);
    if (!found) return;
    const { port, data, spec } = found;
    const next =
        spec.kind === 'mult'
            ? data.value * Math.pow(spec.step, dir)
            : data.value + spec.step * dir;
    commit(port.nodeId, port.portName, clamp(next, data.min, data.max));
    focusedBrushTool()?.refreshHoverOverlay();
}

/** Set an absolute value (used by drag scrubs). */
function setBrushParam(role: Role, absolute: number): void {
    const found = findScalarPort(role);
    if (!found) return;
    const { port, data } = found;
    commit(port.nodeId, port.portName, clamp(absolute, data.min, data.max));
}

/** Transient state held while a canvas size drag is in flight. Module-level
 *  because there's only ever one active drag at a time, gated by
 *  pointer capture. The cursor preview is *anchored* at the pointerdown
 *  position so only size changes visibly during the drag. */
type SizeDragState = {
    startVal: number;
    anchorX: number;
    anchorY: number;
    /** The outline's on-screen diameter at `startVal`, in CSS px, and the
     *  pointer travel when it was measured: `null` until a cursor preview
     *  exists to measure. */
    outline: { diameter: number; dx: number } | null;
};
let sizeDrag: SizeDragState | null = null;

/** On-screen diameter, in CSS px, of the brush outline the focused brush
 *  tool last drew, or `null` when it has none to measure. Measured through
 *  `canvasToScreen`, which carries zoom, view rotation and the device pixel
 *  ratio, so it is in the same unit as the pointer's travel. Uses the extent
 *  before the dab's half-pixel floor, which keeps tracking the size below it. */
function outlineDiameter(): number | null {
    const preview = focusedBrushTool()?.lastPreview;
    const el = app.canvasEl;
    if (!preview || !el) return null;
    const { cx, cy, info } = preview;
    const a = canvasToScreen(cx, cy, el);
    const b = canvasToScreen(cx + 2 * info.nominalHalfExtent[0], cy, el);
    const diameter = Math.hypot(b.x - a.x, b.y - a.y);
    return diameter > 0 ? diameter : null;
}

/** The size whose outline is `dx` CSS px wider on screen than an outline
 *  `diameter` px across at size `start`: the outline's edge follows the
 *  pointer at any zoom, as in Krita's canvas resize
 *  (`KisToolFreehand::continueAlternateAction`, KDE/Krita,
 *  https://invent.kde.org/graphics/krita). */
function sizeFollowingPointer(start: number, dx: number, diameter: number, min: number, max: number): number {
    return clamp((start * (diameter + dx)) / diameter, min, max);
}

export function registerBrushParamActions() {
    actions.register({
        id: 'brushSizeUp',
        handler: () => adjustBrushParam('size', +1),
    });

    actions.register({
        id: 'brushSizeDown',
        handler: () => adjustBrushParam('size', -1),
    });

    actions.register({
        id: 'brushSizeAdjust',
        type: 'hold',
        handler: (ctx) => {
            const found = findScalarPort('size');
            if (!found) {
                sizeDrag = null;
                return;
            }
            const { value, min } = found.data;
            // A size below the floor (a brush saved at 0) has no outline to
            // scale from: lift it to the floor and measure that outline once
            // the next preview draws it.
            if (value < min) setBrushParam('size', min);
            sizeDrag = {
                startVal: Math.max(value, min),
                anchorX: typeof ctx.x === 'number' ? ctx.x : 0,
                anchorY: typeof ctx.y === 'number' ? ctx.y : 0,
                outline: null,
            };
            if (value >= min) {
                const diameter = outlineDiameter();
                if (diameter !== null) sizeDrag.outline = { diameter, dx: 0 };
            }
        },
        onMove: (_ctx, e, dx) => {
            if (!sizeDrag) return;
            const found = findScalarPort('size');
            if (!found) return;
            // With no outline measured yet the size holds still until the
            // preview pushed below resolves, rather than moving at some other
            // rate in the meantime.
            if (!sizeDrag.outline) {
                const diameter = outlineDiameter();
                if (diameter !== null) sizeDrag.outline = { diameter, dx };
            }
            const { outline } = sizeDrag;
            if (outline) {
                const { min, max } = found.data;
                setBrushParam(
                    'size',
                    sizeFollowingPointer(sizeDrag.startVal, dx - outline.dx, outline.diameter, min, max),
                );
            }
            // Re-render the on-canvas brush cursor preview so the circle
            // grows/shrinks live during the drag. Anchored at the start
            // position; pose comes from the live event, matching the
            // normal hover preview (pressure forced to 1 by `cursorPose`
            // so both paths show the same brush extent). Routed through the
            // focused instance's brush tool (own session, own hover state).
            void runHook(
                focusedBrushTool()?.pushHoverOverlay(
                    cursorPose(e),
                    sizeDrag.anchorX,
                    sizeDrag.anchorY,
                ),
            );
        },
        deactivate: () => {
            sizeDrag = null;
        },
    });
}
