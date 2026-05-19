import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import { toolRegistry } from '../tools/registry';
import type { SaveBundle } from '../storage/saveDocument';

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
    blendModeDisplayNames = $state<Record<string, string>>({});
    modifierDisplayNames = $state<Record<string, string>>({});
    layerKindDisplayNames = $state<Record<string, string>>({});

    toolDisplayName(id: string): string {
        return this.toolDisplayNames[id] ?? id;
    }
    veilDisplayName(id: string): string {
        return this.veilDisplayNames[id] ?? id;
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
            try {
                const tree = JSON.parse(this.handle.layer_tree());
                this.layerTree = Array.isArray(tree) ? tree : [];
            } catch { this.layerTree = []; }
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
