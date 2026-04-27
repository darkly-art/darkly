import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';
import { rebuildClickIndex } from './actions/triggers';
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

    // Register actions once, then wire up hotkeys + mouse-click index.
    registerActions();
    registerHotkeys();
    rebuildClickIndex();

    // Re-derive triggers whenever config mutates so rebinds in Settings (or
    // a preset switch) take effect immediately. Both rebuilds are cheap.
    config.onChange(() => {
        registerHotkeys();
        rebuildClickIndex();
    });

    // Push theme colors to WASM now that `app.handle` exists.
    theme.pushToWasm();

    return handle;
}

// HMR'ing this module would create a second WASM engine with a fresh undo
// stack. Force a full reload instead.
if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
