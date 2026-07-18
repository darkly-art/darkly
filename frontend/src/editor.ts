import init from '../wasm/pkg/darkly_wasm';
import { config } from './config/store.svelte';
import { registerHotkeys } from './config/hotkeys.svelte';
import { registerActions } from './actions';
import { rebuildClickIndex } from './actions/triggers';
import { theme } from './state/theme.svelte';
import { pixelFilter } from './state/pixelFilter.svelte';
import { DarklyInstance, setActiveInstance, getActiveInstance } from './state/app.svelte';
import { freshDocument } from './state/freshDocument';
import { createHandle } from './state/session';
import { fontLibrary } from './state/font_library.svelte';
import type { Engine } from './engine/protocol';
import { setupModifierCursorTracking } from './tools/modifier_cursor';
import { setupToolSessionRejectionGuard } from './tools/tool_session';
import { setupColorPickerModifierTracking } from './tools/colorpicker_cursor';
import { setupCloneSourceModifierTracking } from './tools/clone_source_cursor';
import { setupHeldModsTracking } from './actions/held_mods';
import { autosave } from './state/autosave.svelte';
import { recovery } from './state/recovery.svelte';
import { processRecording } from './recording/recorder.svelte';

let processInitialized = false;

/** Process-level setup: WASM module load, config load, theme sync,
 *  action+hotkey registration. Idempotent — safe to call multiple times.
 *  The multi-tab shell calls this once at boot before opening any tabs.
 *
 *  WASM init happens FIRST because `config.init()` calls into WASM exports
 *  (`config_schema`, `config_base_names`) — those would throw with
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

    // Own the canonical held-modifier string once, window-level. The
    // picker + clone set-source cursors subscribe to it (via
    // `onHeldModsChange`) rather than each tracking modifiers themselves.
    // Idempotent.
    setupHeldModsTracking();

    // Shared modifier-cursor machinery: window-level pointer tracking
    // (pointer-down gate + last on-canvas position) that both engagement
    // modules below consume. Idempotent.
    setupModifierCursorTracking();

    // Window-level backstop that swallows an unhandled ToolSessionCancelled —
    // the safety net for any bare `void tool.asyncHook()` spawn that skipped
    // `runHook`. Idempotent.
    setupToolSessionRejectionGuard();

    // Wire the color-picker cursor so it engages as soon as the held
    // modifier resolves to `sampleColor` with a paint tool active (not just
    // on pointerdown). Idempotent.
    setupColorPickerModifierTracking();

    // Same for the Clone brush's set-source cursor — arms the crosshair
    // while the held modifier resolves to `setCloneSource` with a clone
    // brush active. Idempotent.
    setupCloneSourceModifierTracking();

    // Autosave + crash recovery. `recovery.init()` registers this browser
    // session (heartbeat + clean-exit handler) and, if a prior session
    // crashed with unsaved work, prompts to restore it. `autosave.start()`
    // arms the snapshot interval + tab-switch hook.
    autosave.start();
    void recovery.init();

    // Process recording (timelapse). Watches the shell for tab lifecycle
    // and config for the enabled/interval/resolution settings; each tab's
    // render loop drains captured frames via `processRecording.pollFrame`.
    processRecording.start();

    processInitialized = true;
}

/** Options for {@link createInstance}. */
export interface CreateInstanceOptions {
    /** Seed a fresh document with its default background layer (the
     *  deploy-flavor's {@link freshDocument} initial layer — the demo
     *  background image or the app's black fill). Done **before** the engine is
     *  published to `instance.engine`, so any `$effect` that watches
     *  `app.engine` sees a fully-bootstrapped engine — no
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
 *  **Publish order matters**: `instance.engine = engine` is the *last*
 *  thing that happens before `onHandleReady` fires. Every bootstrap
 *  mutation — registry load, name application, optional bg seed —
 *  completes first, so reactive consumers that subscribe on the engine
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

    const engine = await createHandle(canvas, docWidth, docHeight);

    // Display-name maps describe the WASM core's process-global registries —
    // identical for every instance, but loading them per-instance keeps the
    // instance self-contained (no shell-level "registry source" coupling).
    await instance.loadRegistries(engine);

    // Replay the personal font library into this fresh handle so its engine's
    // font collection matches every other tab's before the first frame — the
    // single chokepoint every new handle passes through.
    await fontLibrary.registerIntoHandle(engine);

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
        engine.api.setDocumentName({ name: instance.pendingName });
        instance.pendingName = null;
    }

    // Seed the default background layer for fresh docs. Done before
    // publishing the engine so any reactive consumer that fires on
    // `app.engine` becoming truthy reads a doc that already has its
    // bg layer — eliminates the "refresh after mutation" race the
    // LayerPanel would otherwise hit.
    if (options.seedBackground) {
        const bg = await engine.api.addRaster({ anchor: null });
        freshDocument.fillInitialLayer(engine, bg);
        instance.selectLayer(bg);
    }

    instance.canvasEl = canvas;
    instance.docW = docWidth;
    instance.docH = docHeight;
    instance.engine = engine;

    // Fire the one-shot `onHandleReady` hook (used by the Open
    // Document flow to load a `.darkly` payload into a freshly-opened
    // tab once its async handle bootstrap completes).
    if (instance.onHandleReady) {
        const cb = instance.onHandleReady;
        instance.onHandleReady = null;
        cb(engine);
    }
    return instance;
}

/** Populate a freshly-booted instance with the deploy-flavor's default
 *  starter content (the demo build's hidden veils, or nothing for the app
 *  build) — see {@link freshDocument}. Caller decides when to invoke
 *  (skipped for tabs that load existing documents). Living as a free
 *  function (not a `DarklyInstance` method) keeps "what's in a fresh tab"
 *  at the application layer — the engine itself stays opinion-free. */
export function seedFreshDocument(instance: DarklyInstance, docW: number, docH: number): void {
    if (!instance.engine) return;
    freshDocument.seedVeils(instance, docW, docH);
}

/** Single-instance boot path used by the standalone (non-multi-tab) host.
 *  Creates one `DarklyInstance`, makes it the active one, returns its
 *  handle. CanvasView calls this on mount. */
export async function initEditor(canvas: HTMLCanvasElement): Promise<Engine> {
    // If a prior boot already created an instance (e.g. via HMR or a host
    // that pre-registers one), reuse it instead of orphaning the engine.
    const existing = getActiveInstance();
    if (existing?.engine) {
        return existing.engine;
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
    return instance.engine!;
}

// HMR'ing this module would create a second WASM engine with a fresh undo
// stack. Force a full reload instead.
if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
