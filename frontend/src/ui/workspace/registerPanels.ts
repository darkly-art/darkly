/**
 * Registers the concrete top-level panels. Imported once for its side effects
 * (like `tools/index`). A new panel = one more `registerPanel` call here.
 */

import { registerPanel } from './panelTypes';
import LayerPanel from '../layers/LayerPanel.svelte';
import PropertiesPanel from '../properties/PropertiesPanel.svelte';
import DocumentPanel from '../../multi_tab/DocumentPanel.svelte';

// The canvas: a first-class, tileable panel. Non-closable and non-poppable —
// its WebGPU surface can't migrate to another OS window, so pop-out is disabled
// and the cross-window drag guard keeps it in the main workspace.
registerPanel('document', { title: 'Document', component: DocumentPanel, closable: false, poppable: false });
registerPanel('layers', { title: 'Layers', component: LayerPanel, closable: false, poppable: true });
registerPanel('properties', { title: 'Properties', component: PropertiesPanel, closable: false, poppable: true });
