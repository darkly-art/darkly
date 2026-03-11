import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { copyToSystemClipboard, readImageFromClipboard } from './clipboard';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { app } from './state/app.svelte';
import { toolRegistry } from './tools/registry';
import { MIN_SIZE, MAX_SIZE, SIZE_STEP } from './tools/brush.svelte';

let initialized = false;

export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    if (!initialized) {
        await init();
        config.init();
        initialized = true;
    }

    const docWidth = config.get('canvas.width') as number;
    const docHeight = config.get('canvas.height') as number;
    const handle = await DarklyHandle.create(canvas, docWidth, docHeight);

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
        selectAll:       () => app.handle?.select_all(),
        clearSelection:  () => app.handle?.clear_selection(),
        clearSelectionContents: () => {
            if (app.activeLayerId != null) {
                app.handle?.clear_selection_contents(app.activeLayerId);
            }
        },
        invertSelection: () => app.handle?.invert_selection(),
        copy: () => {
            if (!app.handle || app.activeLayerId == null) return;
            const result = app.handle.copy(app.activeLayerId);
            if (result && result.rgba) {
                copyToSystemClipboard(result.rgba, result.width, result.height);
            }
        },
        cut: () => {
            if (!app.handle || app.activeLayerId == null) return;
            const result = app.handle.cut(app.activeLayerId);
            if (result && result.rgba) {
                copyToSystemClipboard(result.rgba, result.width, result.height);
            }
            app.requestFrame();
        },
        paste: () => {
            if (!app.handle) return;
            readImageFromClipboard().then(clip => {
                if (!clip || !app.handle) return;
                // Center the pasted image in the document.
                const docW = config.get('canvas.width') as number;
                const docH = config.get('canvas.height') as number;
                const ox = Math.round((docW - clip.width) / 2);
                const oy = Math.round((docH - clip.height) / 2);
                const activeId = app.activeLayerId ?? -1;
                const layerId = app.handle.paste_image(
                    clip.width, clip.height, clip.rgba, ox, oy, activeId,
                );
                app.activeLayerId = layerId;
                app.refreshLayerTree();
                app.requestFrame();
            });
        },
        pasteInPlace: () => {
            if (!app.handle) return;
            const activeId = app.activeLayerId ?? -1;
            const layerId = app.handle.paste_in_place(activeId);
            if (layerId >= 0) {
                app.activeLayerId = layerId;
                app.refreshLayerTree();
                app.requestFrame();
            }
        },
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
