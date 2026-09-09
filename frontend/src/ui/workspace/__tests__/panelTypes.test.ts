import { describe, it, expect } from 'vitest';
import { registerPanel, resolvePanel, isPanelRegistered, registeredPanels } from '../panelTypes';
import { isAnchorGroup, KNOWN_PANEL_TYPES, type PanelType } from '../tree';

/**
 * The anchor rule is stated in `tree.ts` rather than read off `PanelMeta.movable`
 * because the workspace store loads the tree at module scope, before
 * `registerPanels` has run; asking the registry then throws with
 * "No panel registered for type 'document'". This file is what keeps the two
 * statements of the same fact from drifting.
 */

// Registering the real panels would pull in Svelte components, which Vitest's
// node environment cannot render. The registry only stores what it is given, so
// stand-in components are enough to assert the flags agree.
const stub = {} as never;
const PANELS: { type: PanelType; movable: boolean }[] = [
    { type: 'document', movable: false },
    { type: 'layers', movable: true },
    { type: 'properties', movable: true },
    { type: 'color', movable: true },
];

for (const { type, movable } of PANELS) {
    if (!isPanelRegistered(type)) {
        registerPanel(type, { title: type, component: stub, closable: true, poppable: true, movable });
    }
}

describe('the anchor rule agrees with the panel registry', () => {
    it.each(PANELS)('$type', ({ type }) => {
        // A non-movable panel is exactly an anchor: it renders no tab bar, so a
        // tab docked beside it would be unreachable.
        expect(isAnchorGroup([type])).toBe(!resolvePanel(type).movable);
    });

    it('a group is an anchor if any of its tabs is', () => {
        expect(isAnchorGroup(['layers', 'document'])).toBe(true);
        expect(isAnchorGroup(['layers', 'properties'])).toBe(false);
        expect(isAnchorGroup([])).toBe(false);
    });
});

describe('the known-panel list agrees with the panel registry', () => {
    // `stripUnknownTabs` drops persisted tabs the list does not name, so a
    // panel registered but missing here would vanish from every saved layout.
    it('names exactly the registered panels', () => {
        expect([...KNOWN_PANEL_TYPES].sort()).toEqual(registeredPanels().sort());
    });
});
