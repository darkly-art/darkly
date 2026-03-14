import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';
import { checkGpu } from './gpu';
import { toast } from './state/toast.svelte';

let initialized = false;

export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    if (!initialized) {
        await init();
        config.init();
        initialized = true;
    }

    // Detect software rendering before creating the engine so the Rust
    // side knows whether to use reduced-resolution veil rendering.
    const gpuCheck = await checkGpu();
    toast.show(gpuCheck.level, gpuCheck.message, gpuCheck.level === 'success' ? 3000 : undefined);

    const docWidth = config.get('canvas.width') as number;
    const docHeight = config.get('canvas.height') as number;
    const handle = await DarklyHandle.create(canvas, docWidth, docHeight, gpuCheck.isSoftware);

    // Register actions once, then wire up hotkeys from config
    registerActions();
    registerHotkeys();

    return handle;
}
