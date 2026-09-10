import { describe, it, expect, beforeEach } from 'vitest';
import { WorkspaceStore } from '../workspaces.svelte';
import { registerPanel, isPanelRegistered } from '../panelTypes';
import { collectPanelTypes, groupHolding, isAnchorGroup, type PanelType } from '../tree';

// Registering the real panels would pull in Svelte components, which the node
// environment cannot render; the store only consults the registry's flags.
const stub = {} as never;
const PANELS: { type: PanelType; closable: boolean; movable: boolean }[] = [
    { type: 'document', closable: false, movable: false },
    { type: 'layers', closable: false, movable: true },
    { type: 'properties', closable: false, movable: true },
    { type: 'color', closable: true, movable: true },
];
for (const { type, closable, movable } of PANELS) {
    if (!isPanelRegistered(type)) registerPanel(type, { title: type, component: stub, closable, poppable: true, movable });
}

let store: WorkspaceStore;
const main = () => store.getWorkspace(0)!.layout.root;

beforeEach(() => {
    // No `localStorage` in node, so the store starts from the default layout:
    // the canvas beside a Layers/Properties column, no color panel.
    store = new WorkspaceStore();
});

describe('panel visibility', () => {
    it('a_closed_panel_opens_into_the_first_dockable_group_never_the_anchor', () => {
        expect(store.isPanelOpen('color')).toBe(false);

        store.togglePanel('color');

        expect(store.isPanelOpen('color')).toBe(true);
        const group = groupHolding(main(), 'color')!;
        expect(isAnchorGroup(group.state.tabs)).toBe(false);
        expect(group.state.tabs[group.state.activeTabIndex]).toBe('color');
    });

    it('toggling_an_open_panel_closes_it_and_prunes_the_tree', () => {
        store.togglePanel('color');
        const before = collectPanelTypes(main()).filter((t) => t !== 'color');

        store.togglePanel('color');

        expect(store.isPanelOpen('color')).toBe(false);
        expect(collectPanelTypes(main())).toEqual(before);
    });

    it('opening_an_already_open_panel_raises_its_tab', () => {
        store.openPanel('color');
        const group = groupHolding(main(), 'color')!;
        store.setActiveTab(0, group.id, group.state.tabs.find((t) => t !== 'color')!);
        expect(groupHolding(main(), 'color')!.state.tabs[groupHolding(main(), 'color')!.state.activeTabIndex]).not.toBe('color');

        store.openPanel('color');

        const after = groupHolding(main(), 'color')!;
        expect(after.state.tabs[after.state.activeTabIndex]).toBe('color');
        expect(collectPanelTypes(main()).filter((t) => t === 'color')).toHaveLength(1);
    });

    it('a_non_closable_panel_refuses_to_close', () => {
        expect(store.isPanelOpen('layers')).toBe(true);

        store.closePanel('layers');
        store.togglePanel('layers');

        expect(store.isPanelOpen('layers')).toBe(true);
    });
});
