import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';
import { rebuildClickIndex } from './actions/triggers';
import { theme } from './state/theme.svelte';
import { DarklyInstance, setActiveInstance, getActiveInstance } from './state/app.svelte';
import { createHandle } from './state/session';

let processInitialized = false;

/** Process-level setup: WASM module load, config load, theme sync,
 *  action+hotkey registration. Idempotent — safe to call multiple times.
 *  The multi-tab shell calls this once at boot before opening any tabs.
 *
 *  WASM init happens FIRST because `config.init()` calls into WASM exports
 *  (`config_schema`, `config_preset_names`) — those would throw with
 *  "Cannot read properties of undefined" if the module hadn't loaded yet. */
export async function ensureProcessInit(): Promise<void> {
    if (processInitialized) return;
    await init();
    await config.init();
    // Theme subscribes to config in its module; trigger an initial sync so
    // body class and WASM preview colors match `ui.theme` from startup.
    theme.syncFromConfig();

    config.onChange(() => {
        registerHotkeys();
        rebuildClickIndex();
    });
    processInitialized = true;
}

/** Create + initialise a `DarklyInstance` bound to `canvas`. Constructs a
 *  fresh `DarklyHandle` via the shared `DarklySession`, populates registry
 *  display-name maps, and runs idempotent action/hotkey registration. The
 *  caller may pass a pre-allocated instance (the multi-tab shell does this
 *  so the instance shows up in the tab strip before its async handle is
 *  ready); otherwise a new one is constructed.
 *
 *  Does NOT touch `setActiveInstance` — the caller decides focus. */
export async function createInstance(
    canvas: HTMLCanvasElement,
    docWidth: number,
    docHeight: number,
    instance: DarklyInstance = new DarklyInstance(),
): Promise<DarklyInstance> {
    await ensureProcessInit();

    const handle = await createHandle(canvas, docWidth, docHeight);

    // Display-name maps describe the WASM core's process-global registries —
    // identical for every instance, but loading them per-instance keeps the
    // instance self-contained (no shell-level "registry source" coupling).
    instance.loadRegistries(handle);

    // Action/hotkey registration is process-wide but reads the active
    // instance via the `app` proxy. Calling it here is idempotent.
    registerActions();
    registerHotkeys();
    rebuildClickIndex();

    instance.canvasEl = canvas;
    instance.handle = handle;
    // Apply the shell's "Untitled N" suggestion if one was stashed
    // before the async handle init. The engine's own default is
    // plain "Untitled" — without this the first tab-strip read would
    // race the rename.
    if (instance.pendingName !== null) {
        handle.set_document_name(instance.pendingName);
        instance.pendingName = null;
    }
    return instance;
}

/** Single-instance boot path used by the standalone (non-multi-tab) host.
 *  Creates one `DarklyInstance`, makes it the active one, returns its
 *  handle. CanvasView calls this on mount. */
export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    // If a prior boot already created an instance (e.g. via HMR or a host
    // that pre-registers one), reuse it instead of orphaning the engine.
    const existing = getActiveInstance();
    if (existing?.handle) {
        return existing.handle;
    }

    const docWidth = config.get('canvas.width') as number;
    const docHeight = config.get('canvas.height') as number;
    const instance = await createInstance(canvas, docWidth, docHeight);
    setActiveInstance(instance);
    theme.pushToWasm();
    return instance.handle!;
}

// HMR'ing this module would create a second WASM engine with a fresh undo
// stack. Force a full reload instead.
if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
