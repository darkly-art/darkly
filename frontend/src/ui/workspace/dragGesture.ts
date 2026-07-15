/**
 * Pure gesture reducer for tab dragging. DOM-free and window-agnostic: whichever
 * window physically holds the pointer hit-tests *its own* panels and feeds the
 * result in as a {@link HitTarget}. Because a `HitTarget` carries its own
 * `workspaceId`, within-window and cross-window drags flow through one code
 * path — a target in a different workspace simply yields a cross-window
 * `move-tab`/`dock` commit.
 *
 * The reducer owns the 5px click-vs-drag threshold (so a plain click still just
 * activates the tab), escape/right-button abort, and mode selection. It is
 * unit-tested in isolation; the component wiring around it is not.
 */

import type { PanelType } from './tree';
import type { DockingEdge } from './dropZones';

export const DRAG_THRESHOLD_PX = 5;

/** Recorded on `pointerdown`, before any movement. */
export interface DragStart {
    sourceWorkspaceId: number;
    groupId: number;
    tabType: PanelType;
    tabIndex: number;
    startX: number;
    startY: number;
}

/** What the reporting window found under the pointer. */
export type HitTarget =
    | { kind: 'tab-bar'; workspaceId: number; groupId: number; insertionIndex: number }
    | { kind: 'body'; workspaceId: number; groupId: number; edge: DockingEdge }
    | { kind: 'none' };

/** The current drop intent, painted as a hint by the reporting window. */
export type DragMode =
    | { kind: 'idle' } // below threshold — this is a click, not a drag
    | { kind: 'none' } // dragging but over no valid target
    | { kind: 'reorder'; workspaceId: number; groupId: number; insertionIndex: number }
    | { kind: 'move-tab'; workspaceId: number; groupId: number; insertionIndex: number }
    | { kind: 'dock'; workspaceId: number; groupId: number; edge: DockingEdge }
    | { kind: 'aborted' };

export interface DragState {
    start: DragStart;
    dragging: boolean; // threshold crossed
    aborted: boolean;
    x: number;
    y: number;
    hit: HitTarget;
    mode: DragMode;
}

export type DragInput =
    | { type: 'move'; x: number; y: number; hit: HitTarget }
    | { type: 'up' }
    | { type: 'abort' };

/** The resolved drop, applied by the store against the (source, target) trees. */
export type DragCommit =
    | { kind: 'reorder'; workspaceId: number; groupId: number; tabType: PanelType; toIndex: number }
    | {
          kind: 'move-tab';
          sourceWorkspaceId: number;
          sourceGroupId: number;
          targetWorkspaceId: number;
          targetGroupId: number;
          tabType: PanelType;
          toIndex: number;
      }
    | {
          kind: 'dock';
          sourceWorkspaceId: number;
          sourceGroupId: number;
          targetWorkspaceId: number;
          targetGroupId: number;
          tabType: PanelType;
          edge: DockingEdge;
      };

export function beginDrag(start: DragStart): DragState {
    return {
        start,
        dragging: false,
        aborted: false,
        x: start.startX,
        y: start.startY,
        hit: { kind: 'none' },
        mode: { kind: 'idle' },
    };
}

function selectMode(start: DragStart, hit: HitTarget): DragMode {
    if (hit.kind === 'none') return { kind: 'none' };
    if (hit.kind === 'tab-bar') {
        const sameGroup = hit.workspaceId === start.sourceWorkspaceId && hit.groupId === start.groupId;
        if (sameGroup) return { kind: 'reorder', workspaceId: hit.workspaceId, groupId: hit.groupId, insertionIndex: hit.insertionIndex };
        return { kind: 'move-tab', workspaceId: hit.workspaceId, groupId: hit.groupId, insertionIndex: hit.insertionIndex };
    }
    return { kind: 'dock', workspaceId: hit.workspaceId, groupId: hit.groupId, edge: hit.edge };
}

/** Advance the gesture. Returns the next state and, on a committing `up`, the
 *  resolved drop (else null). */
export function reduceDrag(state: DragState, ev: DragInput): { state: DragState; commit: DragCommit | null } {
    if (state.aborted) return { state, commit: null };

    if (ev.type === 'abort') {
        return { state: { ...state, aborted: true, mode: { kind: 'aborted' } }, commit: null };
    }

    if (ev.type === 'move') {
        const dragging =
            state.dragging ||
            Math.hypot(ev.x - state.start.startX, ev.y - state.start.startY) >= DRAG_THRESHOLD_PX;
        const mode: DragMode = dragging ? selectMode(state.start, ev.hit) : { kind: 'idle' };
        return { state: { ...state, dragging, x: ev.x, y: ev.y, hit: ev.hit, mode }, commit: null };
    }

    // ev.type === 'up'
    if (!state.dragging) return { state, commit: null }; // a click — tab already activated on down
    return { state, commit: commitFor(state) };
}

function commitFor(state: DragState): DragCommit | null {
    const { start, mode } = state;
    switch (mode.kind) {
        case 'reorder':
            return { kind: 'reorder', workspaceId: mode.workspaceId, groupId: mode.groupId, tabType: start.tabType, toIndex: mode.insertionIndex };
        case 'move-tab':
            return {
                kind: 'move-tab',
                sourceWorkspaceId: start.sourceWorkspaceId,
                sourceGroupId: start.groupId,
                targetWorkspaceId: mode.workspaceId,
                targetGroupId: mode.groupId,
                tabType: start.tabType,
                toIndex: mode.insertionIndex,
            };
        case 'dock':
            return {
                kind: 'dock',
                sourceWorkspaceId: start.sourceWorkspaceId,
                sourceGroupId: start.groupId,
                targetWorkspaceId: mode.workspaceId,
                targetGroupId: mode.groupId,
                tabType: start.tabType,
                edge: mode.edge,
            };
        default:
            return null;
    }
}
