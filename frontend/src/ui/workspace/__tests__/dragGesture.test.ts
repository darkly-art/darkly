import { describe, it, expect } from 'vitest';
import { beginDrag, reduceDrag, DRAG_THRESHOLD_PX, type DragStart, type HitTarget } from '../dragGesture';

function start(overrides: Partial<DragStart> = {}): DragStart {
    return { sourceWorkspaceId: 0, groupId: 1, tabType: 'layers', tabIndex: 0, startX: 100, startY: 100, ...overrides };
}

const none: HitTarget = { kind: 'none' };

describe('threshold / click-vs-drag', () => {
    it('stays idle below the threshold and commits nothing on up (a click)', () => {
        let s = beginDrag(start());
        s = reduceDrag(s, { type: 'move', x: 102, y: 101, hit: none }).state;
        expect(s.dragging).toBe(false);
        expect(s.mode.kind).toBe('idle');
        const { commit } = reduceDrag(s, { type: 'up' });
        expect(commit).toBeNull();
    });

    it('crosses into dragging once the threshold is exceeded', () => {
        let s = beginDrag(start());
        s = reduceDrag(s, { type: 'move', x: 100 + DRAG_THRESHOLD_PX + 1, y: 100, hit: none }).state;
        expect(s.dragging).toBe(true);
    });
});

describe('mode selection', () => {
    const dragged = () => reduceDrag(beginDrag(start()), { type: 'move', x: 200, y: 200, hit: none }).state;

    it('reorders when over the source group tab bar', () => {
        const s = reduceDrag(dragged(), {
            type: 'move',
            x: 200,
            y: 200,
            hit: { kind: 'tab-bar', workspaceId: 0, groupId: 1, insertionIndex: 1 },
        }).state;
        expect(s.mode).toEqual({ kind: 'reorder', workspaceId: 0, groupId: 1, insertionIndex: 1 });
    });

    it('moves when over a different group tab bar in the same window', () => {
        const s = reduceDrag(dragged(), {
            type: 'move',
            x: 200,
            y: 200,
            hit: { kind: 'tab-bar', workspaceId: 0, groupId: 2, insertionIndex: 0 },
        }).state;
        expect(s.mode.kind).toBe('move-tab');
    });

    it('moves cross-window when the target is in another workspace', () => {
        const s = reduceDrag(dragged(), {
            type: 'move',
            x: 200,
            y: 200,
            hit: { kind: 'tab-bar', workspaceId: 5, groupId: 9, insertionIndex: 0 },
        }).state;
        expect(s.mode).toMatchObject({ kind: 'move-tab', workspaceId: 5, groupId: 9 });
    });

    it('docks on a body edge', () => {
        const s = reduceDrag(dragged(), {
            type: 'move',
            x: 200,
            y: 200,
            hit: { kind: 'body', workspaceId: 0, groupId: 2, edge: 'right' },
        }).state;
        expect(s.mode).toEqual({ kind: 'dock', workspaceId: 0, groupId: 2, edge: 'right' });
    });
});

describe('commits', () => {
    function drive(hit: HitTarget) {
        let s = beginDrag(start());
        s = reduceDrag(s, { type: 'move', x: 200, y: 200, hit }).state;
        return reduceDrag(s, { type: 'up' }).commit;
    }

    it('commits a reorder with the source tab type', () => {
        expect(drive({ kind: 'tab-bar', workspaceId: 0, groupId: 1, insertionIndex: 2 })).toEqual({
            kind: 'reorder',
            workspaceId: 0,
            groupId: 1,
            tabType: 'layers',
            toIndex: 2,
        });
    });

    it('commits a cross-window move-tab', () => {
        expect(drive({ kind: 'tab-bar', workspaceId: 3, groupId: 7, insertionIndex: 0 })).toMatchObject({
            kind: 'move-tab',
            sourceWorkspaceId: 0,
            sourceGroupId: 1,
            targetWorkspaceId: 3,
            targetGroupId: 7,
            tabType: 'layers',
        });
    });

    it('commits a dock', () => {
        expect(drive({ kind: 'body', workspaceId: 0, groupId: 2, edge: 'bottom' })).toMatchObject({
            kind: 'dock',
            targetGroupId: 2,
            edge: 'bottom',
        });
    });

    it('commits nothing over empty space', () => {
        expect(drive({ kind: 'none' })).toBeNull();
    });
});

describe('abort', () => {
    it('produces no commit after Escape', () => {
        let s = beginDrag(start());
        s = reduceDrag(s, { type: 'move', x: 200, y: 200, hit: { kind: 'body', workspaceId: 0, groupId: 2, edge: 'left' } }).state;
        s = reduceDrag(s, { type: 'abort' }).state;
        expect(s.aborted).toBe(true);
        expect(reduceDrag(s, { type: 'up' }).commit).toBeNull();
    });
});
