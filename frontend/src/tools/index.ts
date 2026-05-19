import { toolRegistry, toolClusterRegistry } from './registry';
import { brushTool } from './brush.svelte';
import { fillTool } from './fill.svelte';
import { gradientTool } from './gradient.svelte';
import { colorPickerTool } from './colorpicker.svelte';
import { rectSelectTool } from './rect_select.svelte';
import { ellipseSelectTool } from './ellipse_select.svelte';
import { lassoSelectTool } from './lasso_select.svelte';
import { polygonSelectTool } from './polygon_select.svelte';
import { magicWandTool } from './magic_wand.svelte';
import { transformTool } from './transform.svelte';

toolRegistry.register(brushTool);
toolRegistry.register(fillTool);
toolRegistry.register(gradientTool);
toolRegistry.register(colorPickerTool);
toolRegistry.register(rectSelectTool);
toolRegistry.register(ellipseSelectTool);
toolRegistry.register(lassoSelectTool);
toolRegistry.register(polygonSelectTool);
toolRegistry.register(magicWandTool);
toolRegistry.register(transformTool);

// Cluster buttons mirror their default tool's icon (rect_select's dashed
// square for selection; fill's bucket for fill) — the cluster owns no
// independent icon, only routing. See `ToolCluster.svelte`.
toolClusterRegistry.register({
    id: 'select',
    toolIds: ['rect_select', 'ellipse_select', 'lasso_select', 'polygon_select', 'magic_wand'],
    defaultToolId: 'rect_select',
    displayName: 'Selection',
});

toolClusterRegistry.register({
    id: 'fill',
    toolIds: ['fill', 'gradient'],
    defaultToolId: 'fill',
    displayName: 'Fill',
});
