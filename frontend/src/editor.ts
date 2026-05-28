import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';
import { rebuildClickIndex } from './actions/triggers';
import { theme } from './state/theme.svelte';
import { pixelFilter } from './state/pixelFilter.svelte';
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

/** Options for {@link createInstance}. */
export interface CreateInstanceOptions {
    /** Seed a fresh document with a single white background layer (the
     *  default for "new tab" flows). Done **before** the handle is
     *  published to `instance.handle`, so any `$effect` that watches
     *  `app.handle` sees a fully-bootstrapped engine — no
     *  refresh-after-mutation race for consumers like `LayerPanel`. */
    seedBackground?: boolean;
}

/** Create + initialise a `DarklyInstance` bound to `canvas`. Constructs a
 *  fresh `DarklyHandle` via the shared `DarklySession`, populates registry
 *  display-name maps, optionally seeds the default background layer, and
 *  runs idempotent action/hotkey registration. The caller may pass a
 *  pre-allocated instance (the multi-tab shell does this so the instance
 *  shows up in the tab strip before its async handle is ready);
 *  otherwise a new one is constructed.
 *
 *  **Publish order matters**: `instance.handle = handle` is the *last*
 *  thing that happens before `onHandleReady` fires. Every bootstrap
 *  mutation — registry load, name application, optional bg seed —
 *  completes first, so reactive consumers that subscribe on handle
 *  becoming non-null read a fully-initialised engine.
 *
 *  Does NOT touch `setActiveInstance` — the caller decides focus. */
export async function createInstance(
    canvas: HTMLCanvasElement,
    docWidth: number,
    docHeight: number,
    instance: DarklyInstance = new DarklyInstance(),
    options: CreateInstanceOptions = {},
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

    // Apply the shell's "Untitled N" suggestion if one was stashed
    // before the async handle init. The engine's own default is
    // plain "Untitled" — without this the first tab-strip read would
    // race the rename.
    if (instance.pendingName !== null) {
        handle.set_document_name(instance.pendingName);
        instance.pendingName = null;
    }

    // Seed the default background layer for fresh docs. Done before
    // publishing the handle so any reactive consumer that fires on
    // `app.handle` becoming truthy reads a doc that already has its
    // bg layer — eliminates the "refresh after mutation" race the
    // LayerPanel would otherwise hit.
    if (options.seedBackground) {
        const bg = handle.add_raster_layer(-1);
        handle.fill_background(bg);
        instance.activeLayerId = bg;
    }

    instance.canvasEl = canvas;
    instance.docW = docWidth;
    instance.docH = docHeight;
    instance.handle = handle;

    // Fire the one-shot `onHandleReady` hook (used by the Open
    // Document flow to load a `.darkly` payload into a freshly-opened
    // tab once its async handle bootstrap completes).
    if (instance.onHandleReady) {
        const cb = instance.onHandleReady;
        instance.onHandleReady = null;
        cb(handle);
    }
    return instance;
}

/** Populate a freshly-booted instance with the default starter content:
 *  the 4 hidden veils new users discover the feature through. Caller
 *  decides when to invoke (skipped for tabs that load existing
 *  documents). Living as a free function (not a `DarklyInstance` method)
 *  keeps "what's in a fresh tab" at the application layer — the engine
 *  itself stays opinion-free. */
export function seedFreshDocument(instance: DarklyInstance, docW: number, docH: number): void {
    if (!instance.handle) return;
    // The veil chain needs a non-zero viewport before `add_veil` will
    // allocate textures; without this `ensure_textures` no-ops and the
    // next call would unwrap on `views`. CanvasView issues its own
    // resize to the surface dims right after, so the only cost is one
    // GPU realloc that's immediately replaced.
    instance.handle.resize(docW, docH);
    instance.addVeil('rainy_glass', { direction: 135, visible: false });
    instance.addVeil('grain',       { speed: 0.05,    visible: false });
    instance.addVeil('lens_blur',   { radius: 0.25,   visible: false });
    instance.addVeil('vhs',         { visible: false });
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
    const instance = await createInstance(canvas, docWidth, docHeight, new DarklyInstance(), {
        seedBackground: true,
    });
    seedFreshDocument(instance, docWidth, docHeight);
    setActiveInstance(instance);
    theme.pushToWasm();
    pixelFilter.syncFromConfig();
    return instance.handle!;
}

// HMR'ing this module would create a second WASM engine with a fresh undo
// stack. Force a full reload instead.
if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
