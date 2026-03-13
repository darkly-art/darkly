import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';

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

    // Register actions once, then wire up hotkeys from config
    registerActions();
    registerHotkeys();

    return handle;
}
