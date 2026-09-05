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
import { advance, layoutWheel, sectorAt, type Hit } from './wheel_geometry';
import type { WheelTree } from './model';

export type MachineState =
    | { kind: 'closed' }
    | {
        kind: 'engaged';
        pointerId: number;
        center: { x: number; y: number };
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

export function reduce(
    state: MachineState,
    event: MachineEvent,
    tree: WheelTree,
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
            const hit = sectorAt(
                layoutWheel(tree, state.path),
                event.x - state.center.x,
                event.y - state.center.y,
            );
            return { state: { ...state, path: advance(state.path, hit), highlight: hit } };
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
