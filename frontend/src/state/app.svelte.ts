import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import { toolRegistry } from '../tools/registry';
import type { SaveBundle } from '../storage/saveDocument';
import { CameraSource } from '../lib/cameraSource';

export interface Color {
    r: number; g: number; b: number; a: number;
}

/**
 * A self-contained Darkly editor: one `DarklyHandle`, one canvas, one
 * document, one set of UI state (active tool, layer selection, view
 * transform, copy callback, frame scheduler, …). Multiple instances can
 * coexist (multi-tab host); a stand-alone embed has just one. The instance
 * has zero awareness of tabs, siblings, or any host that might contain it —
 * tab management is an outer layer (`frontend/src/multi_tab/shell.svelte.ts`)
 * that simply owns a collection of instances.
 *
 * Components throughout the UI import the global `app` proxy (below) instead
 * of holding an instance reference directly; the host swaps which instance
 * `app` resolves to via [`setActiveInstance`].
 */
export class DarklyInstance {
    /** Stable id, useful as a `{#each}` key in the multi-tab shell. */
    readonly id: string =
        typeof crypto !== 'undefined' && 'randomUUID' in crypto
            ? crypto.randomUUID()
            : `instance-${Math.random().toString(36).slice(2)}`;

    handle = $state<DarklyHandle | null>(null);

    /** Initial document name to apply once the WASM handle finishes
     *  bootstrapping. The shell uses this to thread "Untitled N"
     *  through the async handle-init gap — the engine itself defaults
     *  to plain "Untitled", so without this the first read in the
     *  tab strip would race the rename. Cleared by `createInstance`
     *  once it's been pushed through `set_document_name`. */
    pendingName: string | null = null;

    /** Initial canvas dimensions for this tab. When non-null, override
     *  the global `config.get('canvas.width' | 'canvas.height')` that
     *  fresh tabs default to. Set by `shell.open(name, dims)` for
     *  Opens-as-new-tab where the content has its own intrinsic size
     *  (e.g. opening a PNG: canvas matches the image). Consumed once
     *  by `CanvasView.onMount`. */
    pendingDims: { width: number; height: number } | null = null;

    /** Per-tab cached `.darkly` file handle from the FS Access API.
     *  Set after a successful Save As or after opening a file via
     *  `showOpenFilePicker`; subsequent Ctrl+S writes back to the same
     *  file with no picker prompt. Session-only — handles are not
     *  persisted across page reloads in v1 (see plan's "Out of scope"). */
    fileHandle = $state<FileSystemFileHandle | null>(null);

    /** One-shot hook fired by `createInstance` once `handle` is set.
     *  Used by the Open Document flow to load a `.darkly` payload
     *  into a freshly-opened tab. Cleared after firing. */
    onHandleReady: ((handle: DarklyHandle) => void) | null = null;

    // Colors
    foreground = $state<Color>({ r: 0, g: 0, b: 0, a: 255 });
    background = $state<Color>({ r: 255, g: 255, b: 255, a: 255 });

    // Active tool
    activeToolId = $state<string>('brush');

    /** Last activated sub-tool per cluster id. Lets a cluster button restore
     *  the user's previous choice on click (e.g. "the last selection tool I
     *  used was lasso"). Populated by a $effect in LeftSidebar that watches
     *  activeToolId. */
    lastToolByCluster = $state<Record<string, string>>({});

    // Registry-backed display-name lookups. Each map is populated once at
    // startup from the matching `*_types()` WASM query (see `loadRegistries`).
    // Per-instance payloads (LayerInfo, VeilInfo, ModifierInfo, etc.) carry
    // only the stable `type_id`; UI code resolves the human-readable label
    // through these maps — there is no second copy of the display string.
    toolDisplayNames = $state<Record<string, string>>({});
    veilDisplayNames = $state<Record<string, string>>({});
    voidDisplayNames = $state<Record<string, string>>({});
    blendModeDisplayNames = $state<Record<string, string>>({});
    modifierDisplayNames = $state<Record<string, string>>({});
    layerKindDisplayNames = $state<Record<string, string>>({});

    toolDisplayName(id: string): string {
        return this.toolDisplayNames[id] ?? id;
    }
    veilDisplayName(id: string): string {
        return this.veilDisplayNames[id] ?? id;
    }
    voidDisplayName(id: string): string {
        return this.voidDisplayNames[id] ?? id;
    }
    blendModeDisplayName(id: string): string {
        return this.blendModeDisplayNames[id] ?? id;
    }
    modifierDisplayName(id: string): string {
        return this.modifierDisplayNames[id] ?? id;
    }
    layerKindDisplayName(id: string): string {
        return this.layerKindDisplayNames[id] ?? id;
    }

    /** Populate every registry-backed display-name map from the Rust core in
     *  one pass. Called once during editor init, before action registration
     *  and before `this.handle` is set, so the maps are ready by the time any
     *  UI mounts. */
    loadRegistries(handle: { tool_types(): string; veil_types(): string;
        void_types(): string;
        blend_mode_types(): string; modifier_types(): string;
        layer_kind_types(): string }) {
        const buildMap = (json: string): Record<string, string> => {
            try {
                const arr = JSON.parse(json) as Array<{ type: string; displayName: string }>;
                const m: Record<string, string> = {};
                for (const e of arr) m[e.type] = e.displayName;
                return m;
            } catch {
                return {};
            }
        };
        this.toolDisplayNames = buildMap(handle.tool_types());
        this.veilDisplayNames = buildMap(handle.veil_types());
        this.voidDisplayNames = buildMap(handle.void_types());
        this.blendModeDisplayNames = buildMap(handle.blend_mode_types());
        this.modifierDisplayNames = buildMap(handle.modifier_types());
        this.layerKindDisplayNames = buildMap(handle.layer_kind_types());
    }

    // Active layer
    activeLayerId = $state<number | null>(null);

    // Active veil. Mutually exclusive with activeLayerId — the right
    // sidebar's properties pane shows the props of whichever is non-null.
    activeVeilIndex = $state<number | null>(null);

    // Session "isolate this node" flag. When set, the renderer shows only
    // that node's contribution (e.g. a mask renders grayscale on canvas).
    // Replaces the old per-layer `showMaskLayerId`.
    isolatedNodeId = $state<number | null>(null);

    // Layer tree (read from WASM, refreshed after mutations/undo/redo).
    layerTree = $state<any[]>([]);

    // Mirrors the engine's `thumbnail_version` counter. Bumped from
    // `requestFrame` after each render so any `$derived` that reads
    // a thumbnail (via getNodeThumbnail) re-runs when an async readback
    // lands and the wasm cache is updated.
    thumbnailEpoch = $state(0);

    // Veil list (read from WASM, refreshed after mutations).
    veilList = $state<any[]>([]);

    // View transform (controlled by canvas navigation)
    panX = $state(0);
    panY = $state(0);
    zoom = $state(1.0);
    rotation = $state(0);   // radians
    // Fresh-eyes horizontal flip. Session-only; resets on reload.
    mirrorH = $state(false);

    /** Mirror of the engine's document dimensions, set at handle creation
     *  and on `open_document`. JS coord transforms (`canvasToScreen` /
     *  `screenToCanvas`) recenter around these — reading the engine
     *  per-frame would alias the RefCell borrow held by `render()`. The
     *  Rust side stays the source of truth; this is a read-only cache
     *  kept in sync at the same join points that already mutate the doc. */
    docW = $state(1);
    docH = $state(1);

    // Tool cursor — when non-null, overrides nav cursor on the canvas element.
    toolCursor = $state<string | null>(null);

    // Canvas element reference, set by CanvasView on mount. Tools that
    // are activated outside the canvas's pointer event flow (e.g. paste
    // actions that auto-enter transform mode) read this to build a
    // proper ToolContext.
    canvasEl = $state<HTMLCanvasElement | null>(null);

    selectLayer(id: number | null) {
        // Clicking any layer other than the currently isolated one exits
        // isolation. The user is asking to navigate to a layer that's
        // off-path under the current solo, so the click implies they're
        // done with the solo session — keeping isolation would be a
        // confusing UI deadlock (the click would silently appear to do
        // nothing if the new layer is hidden by isolation). Selecting the
        // same isolated node is a no-op.
        if (this.isolatedNodeId !== null && id !== this.isolatedNodeId) {
            this.handle?.set_isolated_node(0);
            this.isolatedNodeId = null;
            this.requestFrame();
        }
        this.activeLayerId = id;
        this.activeVeilIndex = null;
    }

    selectVeil(index: number | null) {
        this.activeVeilIndex = index;
        this.activeLayerId = null;
    }

    clearSelection() {
        this.activeLayerId = null;
        this.activeVeilIndex = null;
    }

    /** Active webcam (and future screenshare) MediaStream-backed inputs, keyed
     *  by the void layer's id. Each entry holds a `<video>` element, a live
     *  `MediaStream`, and per-frame upload logic. `refreshLayerTree` reaps
     *  entries whose layer no longer exists (covers undo / explicit delete /
     *  document close). Reactive `$state` so the properties panel
     *  re-renders when an entry's `error` string changes. */
    cameraSources = $state<Map<number, CameraSource>>(new Map());

    /** Set of camera-void layer IDs the user has explicitly authorized for
     *  this session. The picker adds the id when a new layer is created;
     *  the "Resume" button in VoidProperties adds it for layers loaded from
     *  a `.darkly`. The reconciler only starts a MediaStream for layers in
     *  this set, so reopening a document doesn't pop a permission prompt or
     *  silently re-enable the camera — the saved last frame is displayed
     *  until the user opts back in. Session-only: never persisted, cleared
     *  on document open / page reload. */
    cameraSessionStarted = $state<Set<number>>(new Set());

    /** Mark a camera void as explicitly user-started for this session.
     *  Idempotent. Triggers a layer-tree refresh so the reconciler picks
     *  the new state up and spins up the MediaStream. */
    markCameraVoidStarted(layerId: number) {
        if (this.cameraSessionStarted.has(layerId)) return;
        this.cameraSessionStarted = new Set(this.cameraSessionStarted).add(layerId);
        this.refreshLayerTree();
    }

    /** Start a MediaStream for a camera void. Called from the reconciler
     *  once the layer is in the tree, the user has opted in (added to
     *  `cameraSessionStarted`), and the void isn't frozen. Idempotent. */
    startCameraVoid(layerId: number) {
        if (!this.handle) return;
        if (this.cameraSources.has(layerId)) return;
        const src = new CameraSource(layerId, this.handle);
        // Reassign the Map so Svelte sees a new identity (Map mutations
        // don't trigger reactivity on their own in current Svelte 5).
        this.cameraSources = new Map(this.cameraSources).set(layerId, src);
        src.start().then(() => {
            // Force a redraw — `error` may have just been set, and we want
            // a frame so the void either starts presenting frames or the
            // VoidProperties notice appears.
            this.cameraSources = new Map(this.cameraSources);
            this.requestFrame();
        });
    }

    /** Stop and unregister a camera void's MediaStream. Called by the delete
     *  action and by `refreshLayerTree` for orphaned entries. */
    stopCameraVoid(layerId: number) {
        const src = this.cameraSources.get(layerId);
        if (!src) return;
        src.stop();
        const next = new Map(this.cameraSources);
        next.delete(layerId);
        this.cameraSources = next;
    }

    /** Surface a camera source's current state to the properties panel.
     *  Returns null when there's no source registered for the id (i.e. the
     *  layer isn't a camera void or the source hasn't been created yet). */
    cameraSourceFor(layerId: number): CameraSource | null {
        return this.cameraSources.get(layerId) ?? null;
    }

    /** Reconcile the live `cameraSources` map against the latest layer tree:
     *  every unfrozen camera void should have a running source, every frozen
     *  / deleted / undone camera void should not. Called from
     *  `refreshLayerTree` after every layer mutation (add / remove / undo /
     *  redo / freeze toggle / document open), so dead MediaStreams are reaped
     *  and the OS camera indicator turns off exactly when the user expects.
     *
     *  Takes the tree as a parameter (rather than reading `this.layerTree`)
     *  so the caller — `refreshLayerTree` — doesn't accidentally read the
     *  same reactive store it's about to write. Reading + writing the same
     *  `$state` inside an effect-tracked code path causes Svelte to loop
     *  the enclosing effect into the infinite-update guard. */
    private reconcileCameraSources(tree: any[]) {
        const desired = new Map<
            number,
            { frozen: boolean; frameDivisor: number; visible: boolean }
        >();
        // Thread `parentVisible` through the walk: a camera void is
        // effectively visible only if every ancestor up to the root is
        // visible, matching the compositor's nested-visibility semantics
        // (see `Doc::effective_visible`). The eye on the camera's own row
        // is necessary but not sufficient — hiding the parent group must
        // also halt uploads.
        const walk = (nodes: any[], parentVisible: boolean) => {
            for (const n of nodes) {
                const selfVisible = n?.visible !== false; // default true
                const effectiveVisible = parentVisible && selfVisible;
                // `type` (not `kind`) is the serde variant tag on `LayerInfo`
                // — set by `#[serde(tag = "type")]` in engine/types.rs. The
                // word `kind` is also used on the inner `ParamInfo`, which
                // is what we confused them for earlier.
                if (n?.type === 'void' && n?.voidType === 'camera') {
                    const params = (n.params ?? []) as Array<{
                        name: string;
                        value?: unknown;
                        default?: unknown;
                    }>;
                    const freezeParam = params.find((p) => p?.name === 'freeze');
                    const frozen =
                        freezeParam?.value === true ||
                        (freezeParam?.value === undefined && freezeParam?.default === true);
                    const divisorParam = params.find((p) => p?.name === 'frame_divisor');
                    const rawDivisor =
                        typeof divisorParam?.value === 'number'
                            ? divisorParam.value
                            : typeof divisorParam?.default === 'number'
                              ? divisorParam.default
                              : 4;
                    const frameDivisor = Math.max(1, Math.floor(rawDivisor));
                    desired.set(n.id, { frozen, frameDivisor, visible: effectiveVisible });
                }
                if (Array.isArray(n?.children)) walk(n.children, effectiveVisible);
            }
        };
        walk(tree, true);

        // Stop sources for layers that disappeared or that are now frozen.
        for (const id of [...this.cameraSources.keys()]) {
            const entry = desired.get(id);
            if (entry === undefined || entry.frozen) {
                this.stopCameraVoid(id);
            }
        }
        // Start sources for camera voids that should be running.
        // Gate on `cameraSessionStarted` so loading a `.darkly` doesn't
        // silently re-enable the camera — the user must explicitly opt in
        // (via the picker for new layers, or the Resume button in
        // VoidProperties for loaded layers).
        for (const [id, { frozen }] of desired) {
            if (!frozen && this.cameraSessionStarted.has(id) && !this.cameraSources.has(id)) {
                this.startCameraVoid(id);
            }
        }

        // Push the latest `frame_divisor` and effective-visibility into
        // every live source. Slider / eye-toggle / parent-hide changes
        // take effect on the next rAF without restarting the MediaStream.
        // Done after start so a freshly-started source picks up the user's
        // current values rather than the constructor defaults.
        for (const [id, { frameDivisor, visible }] of desired) {
            const src = this.cameraSources.get(id);
            if (!src) continue;
            src.setFrameDivisor(frameDivisor);
            src.setVisible(visible);
        }

        // Drop session-started ids whose layer is gone so a future undo
        // that re-adds a different layer at the same id doesn't auto-start
        // by accident.
        let pruned: Set<number> | null = null;
        for (const id of this.cameraSessionStarted) {
            if (!desired.has(id)) {
                pruned ??= new Set(this.cameraSessionStarted);
                pruned.delete(id);
            }
        }
        if (pruned) this.cameraSessionStarted = pruned;
    }

    /** Remove a veil and keep `activeVeilIndex` consistent with the new list. */
    removeVeil(index: number) {
        if (!this.handle) return;
        this.handle.remove_veil(index);
        if (this.activeVeilIndex === index) {
            this.activeVeilIndex = null;
        } else if (this.activeVeilIndex !== null && this.activeVeilIndex > index) {
            this.activeVeilIndex--;
        }
        this.refreshVeilList();
        this.requestFrame();
    }

    /** Reorder a veil and adjust `activeVeilIndex` so the selection follows the move. */
    moveVeil(from: number, to: number) {
        if (!this.handle || from === to) return;
        this.handle.move_veil(from, to);
        const a = this.activeVeilIndex;
        if (a !== null) {
            if (a === from) {
                this.activeVeilIndex = to;
            } else if (from < to && a > from && a <= to) {
                this.activeVeilIndex = a - 1;
            } else if (from > to && a >= to && a < from) {
                this.activeVeilIndex = a + 1;
            }
        }
        this.refreshVeilList();
        this.requestFrame();
    }

    swapColors() {
        const tmp = { ...this.foreground };
        this.foreground = { ...this.background };
        this.background = tmp;
    }

    resetColors() {
        this.foreground = { r: 0, g: 0, b: 0, a: 255 };
        this.background = { r: 255, g: 255, b: 255, a: 255 };
    }

    refreshLayerTree() {
        if (this.handle) {
            let next: any[] = [];
            try {
                const parsed = JSON.parse(this.handle.layer_tree());
                next = Array.isArray(parsed) ? parsed : [];
            } catch { next = []; }
            // Camera voids own a MediaStream + <video>; reconcile the live
            // set against the new tree so freshly-added voids spin up,
            // deleted / frozen / undone voids tear down (turning off the OS
            // camera indicator). Done BEFORE assignment so this method only
            // *writes* `layerTree` (never reads it), keeping it out of any
            // enclosing effect's dependency set — otherwise the write loops
            // back through the effect.
            this.reconcileCameraSources(next);
            this.layerTree = next;
            // Schedule a render frame: callers invoke this after layer
            // mutations (undo/redo, add/remove, drag/drop, etc.), and
            // the engine may have async work pending — dirty-pixel
            // readbacks, content-bounds compute, animation. Without a
            // frame, drain_dirty_thumbnail_readbacks never runs and the
            // layer panel ends up showing pre-mutation thumbnails.
            this.requestFrame();
        }
    }

    refreshVeilList() {
        if (this.handle) {
            try {
                const list = JSON.parse(this.handle.veil_list());
                this.veilList = Array.isArray(list) ? list : [];
            } catch { this.veilList = []; }
        }
    }

    // --- Async copy result callback ---

    private _copyCallback: ((result: any) => void) | null = null;

    /** Register a one-shot callback for when the async copy readback completes. */
    onCopyResult(cb: (result: any) => void) {
        this._copyCallback = cb;
        this.requestFrame();
    }

    // --- Async export result callback ---

    private _exportCallback:
        | ((result: { width: number; height: number; rgba: Uint8Array }) => void)
        | null = null;

    /** Register a one-shot callback for when the async export readback completes. */
    onExportResult(cb: (result: { width: number; height: number; rgba: Uint8Array }) => void) {
        this._exportCallback = cb;
        this.requestFrame();
    }

    // --- Async save result callback ---

    private _saveCallback: ((bundle: SaveBundle) => void) | null = null;

    /** Register a one-shot callback for when the async `.darkly` save
     *  readback completes (manifest JSON + composite RGBA + per-blob
     *  bytes arrive together). The caller PNG-encodes the composite +
     *  thumbnail and assembles the zip; see `storage/saveDocument.ts`. */
    onSaveResult(cb: (bundle: SaveBundle) => void) {
        this._saveCallback = cb;
        this.requestFrame();
    }

    // --- Demand-driven rendering ---

    private _framePending = false;

    /**
     * Number of active UI interactions (panel drags, slider adjustments,
     * etc.) that should suppress continuous animation rendering.  While
     * non-zero, `requestFrame()` still runs one-shot requests (e.g. from
     * tool actions) but will NOT self-schedule the next animation frame.
     * This keeps the main thread free for pointer events so that panels
     * like the brush builder remain responsive during animated veils.
     */
    private _interactionCount = 0;

    /** Call when a sustained UI interaction starts (e.g. node drag). */
    beginInteraction() { this._interactionCount++; }

    /** Call when it ends.  Resumes animation rendering if needed. */
    endInteraction() {
        this._interactionCount = Math.max(0, this._interactionCount - 1);
        if (this._interactionCount === 0) this.requestFrame();
    }

    /** Schedule a render frame if one isn't already pending. */
    requestFrame() {
        if (this._framePending) return;
        this._framePending = true;
        requestAnimationFrame((ts) => {
            this._framePending = false;
            if (!this.handle) return;
            // Push the latest webcam / screenshare frames into their void
            // input textures BEFORE render — handle.render reads from those
            // textures during composite, so a later upload would lag by a
            // frame.
            //
            // The frame count we pass to `tick` is the value the compositor's
            // master counter *will* hold once `handle.render` increments it
            // (which it does inside `update_animations`). Anticipating the
            // increment keeps JS-side divisor gates phase-locked with the
            // Rust-side veil / overlay / void divisors that check the
            // post-increment value — so a camera `divisor=N` fires on the
            // same rAF as a veil `divisor=N`, not one off.
            const nextFrameCount = this.handle.frame_count() + 1;
            for (const src of this.cameraSources.values()) {
                src.tick(nextFrameCount);
            }
            const needsMore = this.handle.render(ts / 1000.0);

            // Sync thumbnail-readback completions into a Svelte-reactive
            // epoch so `$derived` consumers re-run. `!==` (not `>`) so a
            // handle swap that resets the wasm counter to 0 still triggers
            // a re-derivation against the new engine.
            const v = this.handle.thumbnail_version();
            if (v !== this.thumbnailEpoch) this.thumbnailEpoch = v;

            // Per-frame tool hook — async state sync (e.g. GPU readback completion).
            toolRegistry.get(this.activeToolId)?.onFrame?.();

            // Check for completed async copy/cut readback.
            if (this._copyCallback) {
                const result = this.handle.poll_copy_result();
                if (result) {
                    const cb = this._copyCallback;
                    this._copyCallback = null;
                    cb(result);
                }
            }

            // Check for completed async export readback.
            if (this._exportCallback) {
                const result = this.handle.poll_export_result();
                if (result) {
                    const cb = this._exportCallback;
                    this._exportCallback = null;
                    cb(result);
                }
            }

            // Check for completed async `.darkly` save readbacks.
            if (this._saveCallback) {
                const bundle = this.handle.poll_save_result();
                if (bundle) {
                    const cb = this._saveCallback;
                    this._saveCallback = null;
                    cb(bundle);
                }
            }

            // Continue animation loop only when no UI interaction is
            // monopolizing the main thread.  One-shot renders (tool
            // actions, resize, etc.) always go through — only the
            // self-scheduling continuous loop is suppressed.
            const shouldContinue =
                needsMore ||
                this._copyCallback ||
                this._exportCallback ||
                this._saveCallback;
            if (shouldContinue && this._interactionCount === 0) {
                this.requestFrame();
            }
        });
    }
}

// ---------------------------------------------------------------------------
// `app` — global proxy for "the currently focused instance"
// ---------------------------------------------------------------------------
//
// 40+ files do `import { app } from './state/app.svelte'`. To keep them
// untouched, `app` stays a single exported symbol — but it's now a Proxy
// over a swappable underlying instance. Single-instance hosts call
// `setActiveInstance(theLoneInstance)` at boot; the multi-tab shell calls it
// whenever the focused tab changes.

let activeInstance = $state<DarklyInstance | null>(null);

/** Replace the underlying instance that the global `app` proxy resolves to.
 *  Calling this triggers Svelte reactivity on every consumer that reads
 *  `app.<x>` (because the proxy's getter reads the `$state` `activeInstance`,
 *  threading the dependency through). */
export function setActiveInstance(inst: DarklyInstance | null) {
    activeInstance = inst;
}

/** The currently focused instance, or `null` if none has been set. Useful for
 *  the multi-tab shell or boot code that needs the raw instance. */
export function getActiveInstance(): DarklyInstance | null {
    return activeInstance;
}

export const app = new Proxy({} as DarklyInstance, {
    get(_target, prop, _receiver) {
        const inst = activeInstance;
        if (!inst) return undefined;
        const value = (inst as any)[prop];
        // Bind methods so `this` resolves to the instance, not the proxy.
        return typeof value === 'function' ? value.bind(inst) : value;
    },
    set(_target, prop, value) {
        const inst = activeInstance;
        if (!inst) return false;
        (inst as any)[prop] = value;
        return true;
    },
    has(_target, prop) {
        return activeInstance ? prop in activeInstance : false;
    },
});

// `app` resolves through `activeInstance`. HMR'ing this module resets
// `activeInstance` to null, orphaning the running engine. Force a full
// reload instead.
if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
