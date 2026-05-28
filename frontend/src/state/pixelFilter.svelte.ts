/**
 * Pushes the `display.pixelFilter` config value to WASM whenever it changes,
 * so the present shader's canvas-to-screen sampling updates immediately when
 * the user toggles the Settings dropdown — no zoom/pan needed to trigger an
 * upload.
 *
 * Parallel to `theme.svelte.ts`: the module subscribes to `config.onChange`
 * at import time; `editor.ts` imports this module once during process init.
 */
import { app } from './app.svelte';
import { config } from '../config/store.svelte';

let lastMode: string | null = null;

function pushToWasm() {
    if (!app.handle) return;
    const raw = config.get('display.pixelFilter');
    const mode = typeof raw === 'string' ? raw : 'auto';
    if (mode === lastMode) return;
    lastMode = mode;
    app.handle.set_pixel_filter(mode);
    app.requestFrame();
}

export const pixelFilter = {
    syncFromConfig: pushToWasm,
};

config.onChange(pushToWasm);
