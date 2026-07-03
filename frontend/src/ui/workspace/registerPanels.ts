/**
 * Registers the concrete top-level panels. Imported once for its side effects
 * (like `tools/index`). A new panel = one more `registerPanel` call here.
 */

import { registerPanel } from './panelTypes';
import LayerPanel from '../layers/LayerPanel.svelte';
import PropertiesPanel from '../properties/PropertiesPanel.svelte';

registerPanel('layers', { title: 'Layers', component: LayerPanel, closable: false, poppable: true });
registerPanel('properties', { title: 'Properties', component: PropertiesPanel, closable: false, poppable: true });
