import { describe, it, expect, beforeAll } from 'vitest';
import { registerPanel, isPanelRegistered } from '../panelTypes';
import { registerPanelActions, panelActionId } from '../panelActions';
import { workspaces } from '../workspaces.svelte';
import { actions, actionEnablement } from '../../../actions/registry';
import { buildTopMenus } from '../../menu/menuModel';
import type { PanelType } from '../tree';

const stub = {} as never;
const PANELS: { type: PanelType; closable: boolean; movable: boolean }[] = [
    { type: 'document', closable: false, movable: false },
    { type: 'layers', closable: false, movable: true },
    { type: 'properties', closable: false, movable: true },
    { type: 'color', closable: true, movable: true },
];

beforeAll(() => {
    for (const { type, closable, movable } of PANELS) {
        if (!isPanelRegistered(type)) registerPanel(type, { title: type, component: stub, closable, poppable: true, movable });
    }
    registerPanelActions();
});

describe('panel show/hide actions', () => {
    it('every_movable_panel_gets_a_window_menu_action_and_the_anchor_none', () => {
        expect(actions.get(panelActionId('document'))).toBeUndefined();
        for (const type of ['layers', 'properties', 'color'] as const) {
            const a = actions.get(panelActionId(type))!;
            expect(a, type).toBeDefined();
            expect(a.menuPath?.[0].startsWith('Window:')).toBe(true);
        }
    });

    it('the_status_check_follows_whether_the_panel_is_open', () => {
        const a = actions.get(panelActionId('color'))!;
        expect(workspaces.isPanelOpen('color')).toBe(false);
        expect(a.status?.()).toBeUndefined();

        actions.dispatch(panelActionId('color'), {});
        expect(workspaces.isPanelOpen('color')).toBe(true);
        expect(a.status?.()).toBe('fa6-solid:check');

        actions.dispatch(panelActionId('color'), {});
        expect(workspaces.isPanelOpen('color')).toBe(false);
    });

    it('a_non_closable_panel_action_is_disabled_with_a_reason', () => {
        const { enabled, reason } = actionEnablement(actions.get(panelActionId('layers'))!);
        expect(enabled).toBe(false);
        expect(reason).toMatch(/always shown/);
    });

    it('the_window_menu_sits_before_help', () => {
        const titles = buildTopMenus([
            ...actions.all(),
            { id: 'about', displayName: 'About', category: 'help', icon: '', menuPath: ['Help'], handler() {} } as never,
        ]).map((m) => m.title);
        expect(titles.indexOf('Window')).toBeGreaterThan(-1);
        expect(titles.indexOf('Window')).toBe(titles.indexOf('Help') - 1);
    });
});
