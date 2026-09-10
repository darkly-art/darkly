/**
 * The palette popup's gesture state machine.
 *
 * Pure reducer (no DOM, no runes): `reduce` maps (state, event) to the next
 * state plus an optional effect that the caller performs, so the whole
 * gesture is testable with plain object fakes.
 *
 * The wheel exists only mid-gesture: DOWN opens it centered exactly at the
 * pen-down point (never clamped: starting at the hub is the invariant that
 * makes unbounded hit-testing and zero-movement cancel safe; near screen
 * edges the wheel clips instead, and clipped sectors stay selectable by
 * angle), and UP closes it, committing only when a leaf is highlighted.
 *
 * UP carries no coordinates: the commit target is defined as the highlight
 * produced by the last MOVE. The drag dispatcher's release cannot supply an
 * up-position, and defining commit this way pins the degenerate case: a
 * press-and-release with zero movement releases with the hub highlighted and
 * cancels.
 *
 * Whether the summoning trigger is held is not machine state; the drag-chord
 * binding layer decides which pointerdowns reach the machine at all. The
 * pointer id latched at DOWN screens out every other pointer, so a palm
 * touch mid-thread can neither commit nor cancel the gesture.
 */
import { advance, layoutWheel, sectorAt, selectionPath, type Hit } from './wheel_geometry';
import type { WheelTree } from './model';

export type MachineState =
    | { kind: 'closed' }
    | {
        kind: 'engaged';
        pointerId: number;
        center: { x: number; y: number };
        /** The latched pointer's last sample; drives the cursor marker. */
        cursor: { x: number; y: number };
        path: number[];
        highlight: Hit;
    };

export type MachineEvent =
    | { kind: 'down'; pointerId: number; x: number; y: number }
    | { kind: 'move'; pointerId: number; x: number; y: number }
    | { kind: 'up'; pointerId: number }
    | { kind: 'cancel' };

/** `commit` carries the tree path of the leaf to select. */
export type MachineEffect = { kind: 'commit'; path: number[] };

export const CLOSED: MachineState = { kind: 'closed' };

const NO_WIDTHS: ReadonlyMap<string, number> = new Map();

/** `widths` carries each label's rendered length so the layout can widen a
 *  crowded fan; it defaults to none, which is both the frame before anything
 *  has been measured and the geometry the wheel had before labels could widen
 *  anything at all. */
export function reduce(
    state: MachineState,
    event: MachineEvent,
    tree: WheelTree,
    widths: ReadonlyMap<string, number> = NO_WIDTHS,
): { state: MachineState; effect?: MachineEffect } {
    if (state.kind === 'closed') {
        // MOVE/UP while closed occur in practice: when the open was
        // guard-suppressed, the dispatcher still forwards them. No-ops.
        if (event.kind !== 'down') return { state };
        return {
            state: {
                kind: 'engaged',
                pointerId: event.pointerId,
                center: { x: event.x, y: event.y },
                cursor: { x: event.x, y: event.y },
                path: [],
                highlight: { kind: 'hub' },
            },
        };
    }

    switch (event.kind) {
        case 'cancel':
            return { state: CLOSED };
        case 'down':
            return { state };
        case 'move': {
            if (event.pointerId !== state.pointerId) return { state };
            // The layout the pointer is tested against is the one that was on
            // screen when it moved, i.e. the one the previous path produced.
            // That is what makes "hit-test what is drawn" literally true.
            const hit = sectorAt(
                layoutWheel(tree, state.path, widths,
                    selectionPath(state.path, state.highlight)),
                event.x - state.center.x,
                event.y - state.center.y,
            );
            return {
                state: {
                    ...state,
                    cursor: { x: event.x, y: event.y },
                    path: advance(state.path, hit),
                    highlight: hit,
                },
            };
        }
        case 'up': {
            if (event.pointerId !== state.pointerId) return { state };
            const h = state.highlight;
            if (h.kind === 'sector' && h.sector.node.kind === 'leaf') {
                return { state: CLOSED, effect: { kind: 'commit', path: h.sector.path } };
            }
            return { state: CLOSED };
        }
    }
}
