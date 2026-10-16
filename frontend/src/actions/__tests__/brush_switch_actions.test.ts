import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { registerActions } from '../index';
import { actions, actionEnablement } from '../registry';
import { app, DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import { brushGraph } from '../../state/brush_graph.svelte';

beforeAll(() => {
    registerActions();
});

beforeEach(() => {
    setActiveInstance(new DarklyInstance());
    app.activeToolId = 'fill';
});
afterEach(() => {
    vi.restoreAllMocks();
    setActiveInstance(null);
});

const run = (id: string) => actions.get(id)!.handler({});

describe('cycleRecentBrush action', () => {
    it('is_a_no_op_while_a_stroke_is_in_flight', () => {
        const step = vi.spyOn(brushGraph, 'cycleRecentBrush').mockReturnValue('b');
        app.pointerActive = true;
        run('cycleRecentBrush');
        expect(step).not.toHaveBeenCalled();
        expect(app.activeToolId).toBe('fill');
    });

    it('switches_to_the_brush_tool_only_when_a_step_landed', () => {
        const step = vi.spyOn(brushGraph, 'cycleRecentBrush').mockReturnValue(null);
        run('cycleRecentBrush');
        expect(app.activeToolId).toBe('fill');

        step.mockReturnValue('b');
        run('cycleRecentBrush');
        expect(app.activeToolId).toBe('brush');
    });

    it('is_disabled_when_there_is_nothing_to_cycle_to', () => {
        vi.spyOn(brushGraph, 'canCycleRecentBrush', 'get').mockReturnValue(false);
        expect(actionEnablement(actions.get('cycleRecentBrush')!)).toEqual({
            enabled: false,
            reason: 'No other recent brush',
        });
    });
});

describe('revertBrush action', () => {
    it('reverts_the_brush', () => {
        const revert = vi.spyOn(brushGraph, 'revert').mockResolvedValue();
        run('revertBrush');
        expect(revert).toHaveBeenCalledOnce();
    });

    it('is_disabled_without_unsaved_changes', () => {
        vi.spyOn(brushGraph, 'modified', 'get').mockReturnValue(false);
        expect(actionEnablement(actions.get('revertBrush')!).enabled).toBe(false);
        vi.spyOn(brushGraph, 'modified', 'get').mockReturnValue(true);
        expect(actionEnablement(actions.get('revertBrush')!).enabled).toBe(true);
    });
});
