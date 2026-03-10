import { toolRegistry } from './registry';
import { brushTool } from './brush.svelte';
import { eraserTool } from './eraser.svelte';
import { fillTool } from './fill.svelte';
import { gradientTool } from './gradient.svelte';
import { colorPickerTool } from './colorpicker.svelte';
import { rectSelectTool } from './rect_select.svelte';
import { ellipseSelectTool } from './ellipse_select.svelte';
import { overlayDebugTool } from './overlay_debug.svelte';

toolRegistry.register(brushTool);
toolRegistry.register(eraserTool);
toolRegistry.register(fillTool);
toolRegistry.register(gradientTool);
toolRegistry.register(colorPickerTool);
toolRegistry.register(rectSelectTool);
toolRegistry.register(ellipseSelectTool);
toolRegistry.register(overlayDebugTool);
