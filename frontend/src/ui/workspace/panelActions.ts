/**
 * One show/hide action per registered panel, derived from the registry so a
 * new panel gets its Window menu entry, palette row and bindable hotkey from
 * its `registerPanel` call alone. The action carries its own `doc` (the
 * pattern tool-selection actions use): its documentation is the panel's
 * `PanelMeta`, not an entry in the Rust `actions` catalog.
 */
import { actions } from '../../actions/registry';
import { registeredPanels, resolvePanel } from './panelTypes';
import { workspaces } from './workspaces.svelte';
import type { PanelType } from './tree';

export function panelActionId(type: PanelType): string {
    return `panel.${type}`;
}

export function registerPanelActions(): void {
    registeredPanels().forEach((type, i) => {
        const meta = resolvePanel(type);
        // An anchor (the canvas) is neither shown nor hidden: it has no action.
        if (!meta.movable) return;
        actions.register({
            id: panelActionId(type),
            doc: {
                displayName: meta.title,
                category: 'window',
                description: `Show or hide the ${meta.title} panel`,
                icon: meta.icon ?? 'fa6-solid:table-columns',
            },
            menuPath: [`Window:${i}`],
            enabled: () => meta.closable || `The ${meta.title} panel is always shown`,
            status: () => (workspaces.isPanelOpen(type) ? 'fa6-solid:check' : undefined),
            handler: () => workspaces.togglePanel(type),
        });
    });
}
