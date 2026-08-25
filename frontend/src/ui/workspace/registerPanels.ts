/**
 * Registers the concrete top-level panels. Imported once for its side effects
 * (like `tools/index`). A new panel = one more `registerPanel` call here.
 */

import { registerPanel } from './panelTypes';
import LayerPanel from '../layers/LayerPanel.svelte';
import PropertiesPanel from '../properties/PropertiesPanel.svelte';
import DocumentPanel from '../../multi_tab/DocumentPanel.svelte';
import BrushExplorer from '../brush_explorer/BrushExplorer.svelte';

// The canvas: a fixed anchor. Non-movable → no tab, can't be dragged or tabbed
// into; other panels dock around its edges. Non-poppable (WebGPU can't migrate
// windows) and non-closable.
registerPanel('document', { title: 'Document', component: DocumentPanel, closable: false, poppable: false, movable: false });
registerPanel('layers', { title: 'Layers', component: LayerPanel, closable: false, poppable: true, movable: true });
registerPanel('properties', { title: 'Properties', component: PropertiesPanel, closable: false, poppable: true, movable: true });
// Closable, unlike Layers and Properties: the explorer is somewhere a painter
// goes to pick a brush, not chrome they always want the width for. Closing is
// via the View menu — nothing renders a tab close button today.
registerPanel('brushes', { title: 'Brushes', component: BrushExplorer, closable: true, poppable: true, movable: true });
