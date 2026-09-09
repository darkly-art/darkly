/**
 * Registers the concrete top-level panels. Imported once for its side effects
 * (like `tools/index`). A new panel = one more `registerPanel` call here; its
 * show/hide action, Window menu entry and palette row follow from the
 * registry (`registerPanelActions`).
 */

import { registerPanel } from './panelTypes';
import { registerPanelActions } from './panelActions';
import LayerPanel from '../layers/LayerPanel.svelte';
import PropertiesPanel from '../properties/PropertiesPanel.svelte';
import ColorPanel from '../color/ColorPanel.svelte';
import DocumentPanel from '../../multi_tab/DocumentPanel.svelte';

// The canvas: a fixed anchor. Non-movable → no tab, can't be dragged or tabbed
// into; other panels dock around its edges. Non-poppable (WebGPU can't migrate
// windows) and non-closable.
registerPanel('document', { title: 'Document', component: DocumentPanel, closable: false, poppable: false, movable: false });
registerPanel('layers', { title: 'Layers', component: LayerPanel, closable: false, poppable: true, movable: true });
registerPanel('properties', { title: 'Properties', component: PropertiesPanel, closable: false, poppable: true, movable: true });
registerPanel('color', { title: 'Color', component: ColorPanel, closable: true, poppable: true, movable: true, icon: 'fa6-solid:palette' });

registerPanelActions();
