import { toolRegistry } from './registry';
import { brushTool } from './brush.svelte';
import { eraserTool } from './eraser.svelte';
import { fillTool } from './fill.svelte';
import { gradientTool } from './gradient.svelte';
import { colorPickerTool } from './colorpicker.svelte';

toolRegistry.register(brushTool);
toolRegistry.register(eraserTool);
toolRegistry.register(fillTool);
toolRegistry.register(gradientTool);
toolRegistry.register(colorPickerTool);
