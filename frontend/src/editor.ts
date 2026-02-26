import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { registerHotkeys } from './config/hotkeys.svelte';
import { app } from './state/app.svelte';
import { toolRegistry } from './tools/registry';
import { MIN_SIZE, MAX_SIZE, SIZE_STEP } from './tools/brush.svelte';

let initialized = false;

export const DOC_WIDTH = 900;
export const DOC_HEIGHT = 1600;

export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    if (!initialized) {
        await init();
        initialized = true;
    }
    const handle = await DarklyHandle.create(canvas, DOC_WIDTH, DOC_HEIGHT);

    // Register hotkeys once editor is ready
    initHotkeys();

    return handle;
}

function initHotkeys() {
    // Build tool-switching hotkey actions from the registry.
    const toolActions: Record<string, () => void> = {};
    for (const tool of toolRegistry.all()) {
        toolActions[tool.hotkeyAction] = () => { app.activeToolId = tool.id; };
    }

    registerHotkeys({
        undo:            () => { app.handle?.undo(); app.refreshLayerTree(); },
        redo:            () => { app.handle?.redo(); app.refreshLayerTree(); },
        resetColors:     () => app.resetColors(),
        swapColors:      () => app.swapColors(),
        ...toolActions,
        brushSizeUp:     () => {
            app.brushSize = Math.min(app.brushSize + SIZE_STEP, MAX_SIZE);
        },
        brushSizeDown:   () => {
            app.brushSize = Math.max(app.brushSize - SIZE_STEP, MIN_SIZE);
        },
        opacityUp:       () => {
            app.brushOpacity = Math.min(1.0, app.brushOpacity + 0.1);
        },
        opacityDown:     () => {
            app.brushOpacity = Math.max(0.0, app.brushOpacity - 0.1);
        },
    });
}
