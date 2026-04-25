import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';
import { theme } from './state/theme.svelte';

let initialized = false;

export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    if (!initialized) {
        await init();
        await config.init();
        // Theme subscribes to config in its module; trigger an initial sync so
        // body class and WASM preview colors match `ui.theme` from startup.
        theme.syncFromConfig();
        initialized = true;
    }

    const docWidth = config.get('canvas.width') as number;
    const docHeight = config.get('canvas.height') as number;
    const handle = await DarklyHandle.create(canvas, docWidth, docHeight);

    // Register actions once, then wire up hotkeys from config.
    registerActions();
    registerHotkeys();

    // Re-register tinykeys whenever the config mutates so rebinds from the
    // Settings modal take effect immediately. tinykeys re-registration is
    // cheap; no need to filter to only hotkey-shaped keys.
    config.onChange(() => registerHotkeys());

    // Push theme colors to WASM now that `app.handle` exists.
    theme.pushToWasm();

    return handle;
}
