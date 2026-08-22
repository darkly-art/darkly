import { reportEngineError, type Engine, type EngineState } from '../engine/protocol';
import type { Catalog, CatalogEntry, JsonValue } from '../engine/protocol_gen';
import type { SaveBundle } from '../storage/saveDocument';
import { compute_view_matrices } from '../../wasm/pkg/darkly_wasm';
import { toolRegistry, type Tool } from '../tools/registry';
import { tooltipForAction } from '../config/store.svelte';
import { pollPick } from '../tools/color_pick_sync';
import { SessionEngine, runHook } from '../tools/tool_session';
import { tickColorPickerCursor } from '../tools/colorpicker_cursor';
import { tickCloneSourceCursor } from '../tools/clone_source_cursor';
import { MediaStreamSource, describeMediaError } from '../lib/mediaStreamSource';
import { HttpStreamSource } from '../lib/httpStreamSource';
import type { FrameSource, CaptureKind } from '../lib/frameSource';
import { processRecording } from '../recording/recorder.svelte';
import { freshDocument } from './freshDocument';
import { recentColors } from './recents.svelte';
import { colorToHex } from '../lib/color';
import { newId } from '../lib/id';
import {
    appearedRoots,
    collapsedAncestorsOf,
    indexLayerTree,
    nextActiveAfterRemoval,
    type LayerTreeIndex,
} from './layerTree';

export interface Color {
    r: number; g: number; b: number; a: number;
}

/** Packed `poll_save_result` payload: every byte blob concatenated into one
 *  `bytes` buffer, with the lengths needed to slice them back out. */
interface PackedSaveResult {
    manifestLen: number;
    compositeWidth: number;
    compositeHeight: number;
    compositeLen: number;
    blobs: Array<{ path: string; len: number }>;
    bytes: Uint8Array;
}

/** Reconstruct the {@link SaveBundle} `saveDocument.ts` expects from the packed
 *  protocol result (manifest ++ composite ++ blob0 ++ blob1 ++ … in `bytes`). */
function unpackSaveBundle(p: PackedSaveResult): SaveBundle {
    let off = 0;
    const manifestJson = p.bytes.subarray(off, off + p.manifestLen);
    off += p.manifestLen;
    const compositeRgba = p.bytes.subarray(off, off + p.compositeLen);
    off += p.compositeLen;
    const blobs = p.blobs.map((b) => {
        const bytes = p.bytes.subarray(off, off + b.len);
        off += b.len;
        return { path: b.path, bytes };
    });
    return {
        manifestJson,
        compositeWidth: p.compositeWidth,
        compositeHeight: p.compositeHeight,
        compositeRgba,
        blobs,
    };
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
    readonly id: string = newId('instance');

    engine = $state<Engine | null>(null);

    /** Stable key for this tab's crash-recovery snapshot. Distinct from
     *  `id` so it reads clearly at the recovery-store boundary; repeated
     *  autosaves overwrite one snapshot file per tab. A tab restored from
     *  a snapshot gets a fresh `recoveryId` (it's a new live tab). */
    readonly recoveryId: string = newId('recovery');

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
    onHandleReady: ((engine: Engine) => void) | null = null;

    /** Synchronously-readable mirror of engine state, refreshed from
     *  `engine.render`'s returned snapshot each frame (no per-frame query — it's
     *  a downhill projection of render's one borrow). The single home for every
     *  value the UI caches: frame/thumbnail counters and document bools. UI
     *  consumers that can't `await` — `$derived`, menu `enabled()` gates,
     *  `beforeunload` — read this instead of querying the engine. `$state` so
     *  they re-derive when it changes. Grows as the UI needs more (see Rust
     *  `EngineState`). Null until the first frame renders. */
    engineState = $state<EngineState | null>(null);

    // Colors
    foreground = $state<Color>({ ...freshDocument.foreground });
    background = $state<Color>({ ...freshDocument.background });

    /**
     * The foreground color, recorded as recently used.
     *
     * Tools call this at the point they are about to paint with the color,
     * which is what "recent" means here — as distinct from "scrubbed past in
     * the picker", which the picker's per-`pointermove` writes to
     * `foreground` would otherwise record dozens of times a drag.
     *
     * Reading the color and recording it are the same act, so there is no
     * flag for a tool to forget to set: a new color-using tool records
     * because it needs the color.
     */
    consumeForeground(): Color {
        recentColors.use(colorToHex(this.foreground));
        return this.foreground;
    }

    // Active tool
    activeToolId = $state<string>('brush');

    /** This instance's live tool session — the cancellation-aware engine handle
     *  every tool op routes through. Owned here (not module-global) so each tab
     *  keeps its own session: a background tab finishing async init can't steal
     *  the focused tab's session. Plain field — read imperatively inside hooks,
     *  never reactively. Begun / severed by the `CanvasView` transition effect
     *  via {@link beginToolSession} / {@link killToolSession}. See
     *  `tools/tool_session.ts`. */
    session: SessionEngine | null = null;

    /** Per-instance tool objects, constructed lazily from the registry
     *  descriptors and bound to this instance. Each tab owns its own set, so
     *  tool state (gizmo, placement, hover) never aliases across tabs. */
    #tools = new Map<string, Tool>();

    /** The per-instance {@link Tool} for `id`, constructed on first use (bound
     *  to this instance) and cached. Undefined for an unregistered id. */
    tool(id: string): Tool | undefined {
        const existing = this.#tools.get(id);
        if (existing) return existing;
        const descriptor = toolRegistry.get(id);
        if (!descriptor) return undefined;
        const t = descriptor.create(this);
        this.#tools.set(id, t);
        return t;
    }

    /** Begin a fresh tool session over this instance's engine, killing any prior
     *  one (its parked ops reject on resume). Returns the new session, or null
     *  when there's no engine yet. */
    beginToolSession(): SessionEngine | null {
        this.session?.kill();
        this.session = this.engine ? new SessionEngine(this.engine) : null;
        return this.session;
    }

    /** Sever this instance's tool session and leave none — parked ops reject on
     *  resume. Called on tab close so a hook can't land on a torn-down tab. */
    killToolSession(): void {
        this.session?.kill();
        this.session = null;
    }

    /** Tear this instance down when its tab closes: stop its tool session and
     *  stream sources, free the WASM handle, then drop the engine reference. The
     *  instance owns every consumer of its handle, so it owns their teardown —
     *  the shell just removes it from the strip. Nulling `engine` is what makes
     *  the render loop's `if (!engine) return` guard short-circuit an
     *  already-queued rAF; without it, that frame would call `render` on a freed
     *  handle and throw "Attempt to use a moved value". Order matters: stop the
     *  consumers first, then free, then null last so any synchronous `$state`
     *  reaction observes a fully torn-down instance. Idempotent. */
    dispose(): void {
        this.killToolSession();
        for (const id of [...this.streamSources.keys()]) this.stopStreamSource(id);
        this.engine?.free();
        this.engine = null;
        this.engineState = null;
    }

    /** Monotonic counter the `CanvasView` transition effect watches to re-run a
     *  same-tool activation (paste-into-active-transform: re-pick the floating
     *  without deactivating). Bumped by {@link requestToolReactivation}. */
    toolReactivations = $state(0);

    /** Ask the transition effect to reactivate the current tool (rebind + fresh
     *  `onActivate`, no deactivate). Used by the paste flow when transform is
     *  already active. */
    requestToolReactivation() {
        this.toolReactivations++;
    }

    /** Last activated sub-tool per cluster id. Lets a cluster button restore
     *  the user's previous choice on click (e.g. "the last selection tool I
     *  used was lasso"). Populated by a $effect in LeftSidebar that watches
     *  activeToolId. */
    lastToolByCluster = $state<Record<string, string>>({});

    /** Every registry the Rust core declares, keyed by catalog id ("filters",
     *  "veils", "tools", …). Fetched once at startup (see `loadRegistries`).
     *  Per-instance payloads (LayerInfo, VeilInfo, …) carry only the stable
     *  `type_id`; UI code resolves the human-readable label and the icon
     *  through here, so there is no second copy of either. */
    catalogs = $state<Record<string, Catalog>>({});

    /** `voidType → CaptureKind` for voids backed by a browser MediaStream
     *  (camera / screenshare). Built from the `voids` catalog in
     *  `loadRegistries`; procedural voids are absent. Drives which
     *  `MediaDevices` API to call and is the single source of truth for "is
     *  this a stream-backed void?" across the reconciler, picker, and
     *  properties panel. */
    voidCaptureKind = $state<Map<string, CaptureKind>>(new Map());

    /** Entries of one catalog, or an empty array when it is unknown. */
    entries(catalogId: string): CatalogEntry[] {
        return this.catalogs[catalogId]?.entries ?? [];
    }

    /** One entry by catalog and `type_id`, or `undefined`. */
    entry(catalogId: string, typeId: string): CatalogEntry | undefined {
        return this.entries(catalogId).find((e) => e.type === typeId);
    }

    /** Display label for a `type_id` within a catalog (e.g. `"curves"` →
     *  `"Curves"`), falling back to the id itself when unknown. */
    displayName(catalogId: string, typeId: string): string {
        return this.entry(catalogId, typeId)?.displayName ?? typeId;
    }

    /** The Iconify glyph to render for a tool.
     *
     *  A tool's glyph is registry metadata and lives on its Rust registration.
     *  A descriptor may override it when the glyph depends on live session
     *  state a static registration cannot express — the brush shows the eraser
     *  icon while erase mode is on. This is the single place that precedence
     *  is decided, so no caller branches on a tool id. */
    toolGlyph(typeId: string): string {
        const override = toolRegistry.get(typeId)?.icon;
        const resolved = typeof override === 'function' ? override() : override;
        return resolved ?? this.entry('tools', typeId)?.icon ?? 'fa6-solid:wrench';
    }

    /** A tool button's `title` — its label plus the chord currently bound to
     *  the action that selects it. Both the label and that action id are the
     *  tool's own registry metadata, so resolving them together here keeps the
     *  toolbar and the cluster flyout from each doing the lookup. */
    toolTooltip(typeId: string): string {
        const entry = this.entry('tools', typeId);
        return tooltipForAction(entry?.displayName ?? typeId, entry?.hotkeyAction ?? '');
    }

    /** Populate every registry projection from the Rust core in one pass.
     *  Called once during editor init, before action registration and before
     *  `this.handle` is set, so the catalogs are ready by the time any UI
     *  mounts. */
    async loadRegistries(engine: Engine) {
        const byId: Record<string, Catalog> = {};
        for (const c of (await engine.api.catalogs()) ?? []) byId[c.id] = c;
        this.catalogs = byId;
        // Map each void type to its browser capture API, if any. Voids with a
        // `captureKind` (camera / screenshare) drive the generic MediaStream
        // lifecycle; procedural voids (noise) leave it null and never appear
        // here.
        const capKinds = new Map<string, CaptureKind>();
        for (const v of this.entries('voids')) {
            if (v.captureKind) capKinds.set(v.type, v.captureKind);
        }
        this.voidCaptureKind = capKinds;
    }

    /** Add a veil with a partial overrides record. Param names match the
     *  veil type's registered `name` fields (see each veil's `PARAMS`).
     *  Missing params fall back to registered defaults via the WASM
     *  bridge. Pass `visible: false` to hide the veil after add (the
     *  common starter-veil case). */
    async addVeil(type: string, options: Record<string, unknown> = {}): Promise<void> {
        const engine = this.engine;
        if (!engine) return;
        const { visible, ...params } = options;
        engine.api.addVeil({ veil_type: type, params: params as JsonValue });
        if (visible === false) {
            // `veil_list` returns highest-index first, so the just-added veil
            // sits at index 0 of the array. The list send is enqueued after the
            // add above, so FIFO ordering guarantees it sees the new veil.
            const list = (await engine.api.veilList()) as Array<{ index: number }>;
            const added = list[0];
            if (added) engine.api.setVeilVisible({ index: added.index, visible: false });
        }
        await this.refreshVeilList();
        this.requestFrame();
    }

    // Active layer — the "primary" layer within the selection. Drives the
    // properties panel, paint target, shift-click anchor, and per-row
    // emphasis. Always a member of `selectedLayerIds` when that set is
    // non-empty; null iff the set is empty.
    activeLayerId = $state<number | null>(null);

    // Multi-selection set. Membership is mutated only via `selectLayer`,
    // `toggleLayer`, `extendSelectionTo`, `selectLayers`, and
    // `clearSelection` so the invariant with `activeLayerId` holds.
    selectedLayerIds = $state<Set<number>>(new Set());

    // The layer tree's shape as of the last `refreshLayerTree`. Plain, not
    // `$state`: the reconciler consults the pre-mutation shape to work out which
    // row replaced a deleted one, and reading `layerTree` there would tie the
    // layer panel's `$effect` to the very state the refresh just wrote.
    private treeIndex: LayerTreeIndex = indexLayerTree([]);

    // Active veil. Mutually exclusive with activeLayerId — the right
    // sidebar's properties pane shows the props of whichever is non-null.
    activeVeilIndex = $state<number | null>(null);

    // Session "isolate this node" flag. When set, the renderer shows only
    // that node's contribution (e.g. a mask renders grayscale on canvas).
    // Replaces the old per-layer `showMaskLayerId`.
    isolatedNodeId = $state<number | null>(null);
    private isolationRequestGeneration = 0;

    async setIsolatedNode(id: number | null): Promise<number | null> {
        const engine = this.engine;
        if (!engine) return this.isolatedNodeId;
        const generation = ++this.isolationRequestGeneration;
        let installed: number | null;
        try {
            installed = await engine.api.setIsolatedNode({ id });
        } catch (error) {
            reportEngineError(error);
            return this.isolatedNodeId;
        }
        if (generation === this.isolationRequestGeneration && engine === this.engine) {
            this.isolatedNodeId = installed;
            this.requestFrame();
        }
        return installed;
    }

    // Layer tree (read from WASM, refreshed after mutations/undo/redo).
    layerTree = $state<any[]>([]);

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

    /** Plane-space offset of the canvas window (`Document::canvas_origin`),
     *  mirrored from the engine's `canvas_rect()` query. `(0, 0)` until the
     *  document is cropped/resized with a moved window. `screenToCanvas`
     *  returns plane coords (adds this); `canvasToScreen` subtracts it. Kept
     *  in sync at the same join points as `docW`/`docH`. */
    canvasOriginX = $state(0);
    canvasOriginY = $state(0);

    /** Viewport backing-store size in buffer pixels (`canvas.width/height` =
     *  CSS × DPR). Set by CanvasView on mount and on element resize — the
     *  reactive mirror that lets {@link viewMatrices} stay fresh on resize
     *  without reading the DOM element from a `$derived`. */
    viewportW = $state(1);
    viewportH = $state(1);

    /** The screen↔plane coordinate matrices, derived from the single Rust
     *  source of truth (`compute_view_matrices`) — the JS coordinate path
     *  consumes these instead of re-deriving the transform. 12 floats:
     *  `[screen→plane (6), plane→screen (6)]`, each row-major
     *  `[m00, m01, m02, m10, m11, m12]`. Reactive over every view input
     *  (pan/zoom/rotation/mirror, viewport size, canvas origin, doc dims), so
     *  it can never go stale; pure (no engine borrow), so reading it inside a
     *  pointer event cannot alias the RefCell borrow held by `render()`. */
    viewMatrices: Float32Array = $derived.by(() => {
        if (!this.engine) {
            // Identity (screen→plane, plane→screen) until the engine exists.
            return new Float32Array([1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0]);
        }
        const dpr = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
        return compute_view_matrices(
            this.panX * dpr, this.panY * dpr,
            this.zoom, this.rotation, this.mirrorH,
            this.viewportW, this.viewportH,
            this.canvasOriginX, this.canvasOriginY,
            this.docW, this.docH,
        );
    });

    // Tool cursor — when non-null, overrides nav cursor on the canvas element.
    toolCursor = $state<string | null>(null);

    // Transform-mode context menu: viewport coords where the user right-clicked
    // inside the active transform gizmo, or null when closed. The transform
    // tool sets it; `TransformModeMenu` renders against it (mirrors how
    // `toolCursor` flows tool → reactive UI).
    transformModeMenu = $state<{ x: number; y: number } | null>(null);

    // Canvas element reference, set by CanvasView on mount. Tools reach it
    // through their instance (the `ToolBase.canvasEl` getter); paste actions
    // that auto-enter transform mode check it before requesting activation.
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
            void this.setIsolatedNode(null);
        }
        this.setSoleSelection(id);
    }

    /** Make `id` the entire selection. The single mutator of the
     *  `activeLayerId` / `selectedLayerIds` pair; `selectLayer` layers the
     *  isolation-exit policy on top, which the reconciler deliberately does not
     *  want (it clears isolation only when the isolated node itself dies). */
    private setSoleSelection(id: number | null) {
        this.activeLayerId = id;
        this.selectedLayerIds = id === null ? new Set() : new Set([id]);
        this.activeVeilIndex = null;
    }

    /** The layer-tree node with `id`, searched depth-first through the group
     *  hierarchy, or null if absent. The single home for "resolve a node by id"
     *  so consumers (tools, panels) never re-walk the tree themselves. */
    nodeById(id: number): any | null {
        const walk = (nodes: any[]): any | null => {
            for (const n of nodes) {
                if (n.id === id) return n;
                if (Array.isArray(n.children)) {
                    const found = walk(n.children);
                    if (found) return found;
                }
            }
            return null;
        };
        return walk(this.layerTree);
    }

    /** The active layer's tree node, or null when nothing is selected. */
    get activeNode(): any | null {
        return this.activeLayerId === null ? null : this.nodeById(this.activeLayerId);
    }

    /** The mask modifier id relevant to the active node, or null. Resolves
     *  both cases: the active node is a host that owns a mask, and the
     *  active node *is* a mask modifier — clicking the mask thumbnail makes
     *  the mask the active node, and a mask has no mask child of its own.
     *  Drives the `maskToSelection` enabled guard. */
    get activeMaskId(): number | null {
        const active = this.activeLayerId;
        if (active === null) return null;
        let found: number | null = null;
        const walk = (nodes: any[]): boolean => {
            for (const n of nodes) {
                const mask = n.modifiers?.find((m: any) => m.kind === 'mask') ?? null;
                if (n.id === active) {
                    found = mask ? mask.id : null;
                    return true;
                }
                if (mask && mask.id === active) {
                    found = mask.id;
                    return true;
                }
                if (Array.isArray(n.children) && walk(n.children)) return true;
            }
            return false;
        };
        walk(this.layerTree);
        return found;
    }

    /** Ctrl/Cmd-click router. Adds `id` if absent, removes if present.
     *  When removing the active id, demotes `activeLayerId` to the next
     *  remaining selected id in panel order (or null when empty). */
    toggleLayer(id: number) {
        const next = new Set(this.selectedLayerIds);
        if (next.has(id)) {
            next.delete(id);
            if (this.activeLayerId === id) {
                this.activeLayerId = this.firstInTreeOrder(next);
            }
            this.selectedLayerIds = next;
        } else {
            next.add(id);
            this.selectedLayerIds = next;
            this.activeLayerId = id;
        }
        this.activeVeilIndex = null;
    }

    /** Shift-click router. Selects the inclusive range from the current
     *  active layer (anchor) to `id` in panel order. With no anchor,
     *  degenerates to a plain select. */
    extendSelectionTo(id: number) {
        if (this.activeLayerId === null) {
            this.selectLayer(id);
            return;
        }
        // Visible order only: shift-click spans the rows the user can see, so
        // it never reaches into a collapsed group.
        const order = indexLayerTree(this.layerTree).visibleOrder;
        const anchorIdx = order.indexOf(this.activeLayerId);
        const targetIdx = order.indexOf(id);
        if (anchorIdx < 0 || targetIdx < 0) {
            this.selectLayer(id);
            return;
        }
        const [lo, hi] = anchorIdx <= targetIdx
            ? [anchorIdx, targetIdx]
            : [targetIdx, anchorIdx];
        this.selectedLayerIds = new Set(order.slice(lo, hi + 1));
        // Active follows the click so subsequent shift-clicks extend from
        // where the user is currently pointing — standard Photoshop.
        this.activeLayerId = id;
        this.activeVeilIndex = null;
    }

    /** Replace the multi-selection with `ids`. The last id becomes
     *  active (matches plain-click semantics: focus follows the most
     *  recent touch). Used by batch ops like duplicate that want the
     *  user to land on the freshly-created layers. */
    selectLayers(ids: number[]) {
        if (ids.length === 0) {
            this.clearSelection();
            return;
        }
        if (this.isolatedNodeId !== null) {
            void this.setIsolatedNode(null);
        }
        this.selectedLayerIds = new Set(ids);
        this.activeLayerId = ids[ids.length - 1];
        this.activeVeilIndex = null;
    }

    /** True iff `id` is in the multi-selection. */
    isSelected(id: number): boolean {
        return this.selectedLayerIds.has(id);
    }

    /** Layer-panel row click router. Plain → select, ctrl/cmd → toggle,
     *  shift → extend range. Both LayerItem and LayerGroup call this so
     *  the modifier handling stays in one place. */
    handleLayerRowClick(id: number, e: MouseEvent) {
        if (e.shiftKey) this.extendSelectionTo(id);
        else if (e.ctrlKey || e.metaKey) this.toggleLayer(id);
        else this.selectLayer(id);
    }

    /** Pick the first id in `set` that appears in panel order, skipping rows
     *  inside collapsed groups — the caller is the ctrl-click demotion path, so
     *  the answer should be a row the user can see. Returns null when the set is
     *  empty or none of its ids are still visible. Indexed fresh from the live
     *  tree, since callers may have assigned `layerTree` without a refresh. */
    private firstInTreeOrder(set: Set<number>): number | null {
        if (set.size === 0) return null;
        for (const id of indexLayerTree(this.layerTree).visibleOrder) {
            if (set.has(id)) return id;
        }
        return null;
    }

    /** Reconcile session state that references layer ids against the latest
     *  layer tree: the selection and the isolation target. Ids that no longer
     *  exist (deleted, undone, replaced by a bake result, etc.) drop out, and
     *  when the active row itself disappeared, the row that took its place
     *  becomes active — so no removal path has to know about reselection.
     *  Called from `refreshLayerTree`, which every tree mutation funnels
     *  through, so delete / batch-delete / undo fallout is handled in one place.
     *
     *  `adoptAppeared` asks for the opposite direction: rows that just came into
     *  existence become the selection. Only undo/redo pass it — an ordinary
     *  refresh must never seize the selection just because it noticed a row for
     *  the first time.
     *
     *  Takes the tree as a parameter (rather than reading `this.layerTree`)
     *  so this code path stays write-only on `layerTree` — reading it here
     *  would tie the LayerPanel's `$effect` to the very state the method
     *  just wrote, looping Svelte's update guard. Same pattern as
     *  `reconcileStreamSources(next)`. */
    private reconcileSelection(tree: any[], adoptAppeared = false) {
        const prev = this.treeIndex;
        const next = indexLayerTree(tree);
        this.treeIndex = next;

        // The isolation target is session state pointing at a document node: if
        // it leaves the tree it becomes unreachable from the compositor's root
        // walk, every node tests as off-path, and the canvas goes blank until
        // something happens to reset it. Checked independently of the selection
        // below, because the isolated node can die while a different, still-live
        // layer is active. Krita ends isolation the same way when its isolation
        // root is removed (`KisImage::aboutToRemoveANode`).
        if (this.isolatedNodeId !== null && !next.ids.has(this.isolatedNodeId)) {
            void this.setIsolatedNode(null);
        }

        const survivors = [...this.selectedLayerIds].filter((id) => next.ids.has(id));
        if (survivors.length !== this.selectedLayerIds.size) {
            this.selectedLayerIds = new Set(survivors);
        }

        const appeared = adoptAppeared ? appearedRoots(prev, next) : [];
        if (appeared.length > 0) {
            this.selectedLayerIds = new Set(appeared);
            this.activeLayerId = appeared[0];
            this.activeVeilIndex = null;
        } else {
            const active = this.activeLayerId;
            if (active !== null && !next.ids.has(active)) {
                // Prefer a surviving member of the selection, keeping the rest
                // of it intact; otherwise adopt whatever replaced the dead row.
                const demoted = next.order.find((id) => this.selectedLayerIds.has(id));
                if (demoted !== undefined) this.activeLayerId = demoted;
                else this.setSoleSelection(nextActiveAfterRemoval(prev, next.ids, active));
            }
        }

        // A row the panel doesn't draw reads to the user as "nothing selected",
        // which is the complaint reselection exists to fix. Open whatever hides
        // it, as GIMP's tree view does for every newly selected item.
        if (this.activeLayerId !== null) {
            for (const id of collapsedAncestorsOf(next, this.activeLayerId)) {
                this.engine?.api.setGroupCollapsed({ id, collapsed: false });
            }
        }
    }

    selectVeil(index: number | null) {
        this.activeVeilIndex = index;
        this.activeLayerId = null;
        this.selectedLayerIds = new Set();
    }

    clearSelection() {
        this.activeLayerId = null;
        this.selectedLayerIds = new Set();
        this.activeVeilIndex = null;
    }

    /** Active external-frame void inputs, keyed by the void layer's id. Each
     *  entry is a `FrameSource` — a `MediaStreamSource` (camera / screenshare)
     *  or an `HttpStreamSource` (Blender feed) — owning its per-frame upload
     *  logic. `refreshLayerTree` reaps entries whose layer no longer exists
     *  (covers undo / explicit delete / document close). Reactive `$state` so
     *  the properties panel re-renders when an entry's `error` string changes. */
    streamSources = $state<Map<number, FrameSource>>(new Map());

    /** Set of stream-backed void layer IDs the user has explicitly authorized
     *  for this session. The picker adds the id when a new layer is created;
     *  the "Connect"/"Resume" button in VoidProperties adds it for layers loaded
     *  from a `.darkly` (or after an external stop / disconnect). Reopening a
     *  document does NOT add to this set, so the saved last frame is shown until
     *  the user opts back in — no surprise permission prompt or capture
     *  indicator. Session-only: never persisted, cleared on document open /
     *  page reload. */
    streamSessionStarted = $state<Set<number>>(new Set());

    /** Mark a stream-backed void as explicitly user-started for this session.
     *  Idempotent. Triggers a layer-tree refresh so the reconciler picks the
     *  new state up (drives `showResume` in VoidProperties). The actual source
     *  is started by the gesture via `startStreamSource`, not here. */
    markStreamVoidStarted(layerId: number) {
        if (this.streamSessionStarted.has(layerId)) return;
        this.streamSessionStarted = new Set(this.streamSessionStarted).add(layerId);
        this.refreshLayerTree();
    }

    /** Acquire a `MediaStream` for the given capture kind. MUST be called
     *  synchronously inside a user gesture (before any awaitable round-trip) —
     *  `getDisplayMedia` requires transient activation, which expires if an
     *  `await` runs first. Rejects with a `DOMException` the caller maps via
     *  `describeMediaError`. */
    acquireMediaStream(captureKind: CaptureKind): Promise<MediaStream> {
        if (captureKind === 'display') {
            return navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
        }
        return navigator.mediaDevices.getUserMedia({ video: true, audio: false });
    }

    /** Start a frame source for a stream-backed void, dispatching on capture
     *  kind. `camera` / `display` build a `MediaStreamSource` and adopt (or
     *  acquire) a `MediaStream` — the reconciler no longer starts these; only
     *  gestures do (the picker and the Resume button), which keeps every start
     *  inside a user activation. The picker, which has already `await`ed
     *  `add_void`, passes its in-gesture pre-acquired `stream` (or `acquireError`
     *  if the user cancelled); Resume passes neither and acquires in-gesture
     *  here. `stream` voids (Blender) build an `HttpStreamSource` and connect to
     *  the layer's `url` param immediately — no gesture or permission needed for
     *  a localhost HTTP stream. Idempotent. */
    async startStreamSource(
        layerId: number,
        captureKind: CaptureKind,
        stream?: MediaStream,
        acquireError?: unknown,
    ) {
        if (!this.engine) return;
        // A live healthy source wins — repeat gestures are no-ops. A dead one
        // (`ended`) or one whose acquisition failed (`error` set but never
        // ended, e.g. a denied permission prompt) yields to the retry.
        const existing = this.streamSources.get(layerId);
        if (existing && !existing.ended && !existing.error) return;
        existing?.stop();

        // Both callbacks reassign the map so Svelte sees a new identity —
        // field mutation on a class instance inside a `$state` Map is
        // invisible to reactivity.
        const onEnded = (id: number) => this.onStreamSourceEnded(id);
        const onStatusChange = () => {
            this.streamSources = new Map(this.streamSources);
        };

        if (captureKind === 'stream') {
            const src = new HttpStreamSource(layerId, this.engine, onEnded, onStatusChange);
            src.setMaxSourceDimension(Math.max(this.docW, this.docH));
            this.streamSources = new Map(this.streamSources).set(layerId, src);
            // Connect to the void's configured endpoint (defaults to the add-on's
            // localhost URL). No gesture/permission gate for a localhost stream.
            void src.start(this.streamVoidUrl(layerId));
            this.streamSources = new Map(this.streamSources);
            this.requestFrame();
            return;
        }

        const src = new MediaStreamSource(
            layerId,
            this.engine,
            captureKind,
            onEnded,
            onStatusChange,
        );
        // Cap uploads to the document resolution up front so the very first
        // frame is already downscaled (the reconciler keeps it current after
        // a canvas resize).
        src.setMaxSourceDimension(Math.max(this.docW, this.docH));
        // Register immediately so the properties panel can surface error/Resume
        // state even if acquisition failed. Reassign the Map so Svelte sees a
        // new identity (in-place Map mutation isn't reactive in Svelte 5).
        this.streamSources = new Map(this.streamSources).set(layerId, src);
        // Acquisition failure drops the session opt-in like an external end
        // does, so the error notice AND the Resume affordance both show.
        if (acquireError !== undefined) {
            src.markFailed(describeMediaError(acquireError, captureKind));
            this.clearStreamSessionStarted(layerId);
        } else {
            try {
                const s = stream ?? (await this.acquireMediaStream(captureKind));
                await src.start(s);
            } catch (err) {
                src.markFailed(describeMediaError(err, captureKind));
                this.clearStreamSessionStarted(layerId);
            }
        }
        // Force a redraw — `error` may have just been set, and we want a frame
        // so the void either starts presenting frames or the notice appears.
        this.streamSources = new Map(this.streamSources);
        this.requestFrame();
    }

    /** Current `url` param value for a `stream` void, or the empty string if the
     *  layer / param can't be found (the source then reports a connect error).
     *  Read from the layer tree, where void params live post-reconcile. */
    private streamVoidUrl(layerId: number): string {
        const findUrl = (nodes: any[]): string | null => {
            for (const n of nodes) {
                if (n?.id === layerId && n?.type === 'void') {
                    const p = (n.params ?? []).find((q: any) => q?.name === 'url');
                    const v = p?.value ?? p?.default;
                    return typeof v === 'string' ? v : null;
                }
                if (Array.isArray(n?.children)) {
                    const hit = findUrl(n.children);
                    if (hit !== null) return hit;
                }
            }
            return null;
        };
        return findUrl(this.layerTree) ?? '';
    }

    /** Stop and unregister a stream-backed void's frame source. Called by the
     *  delete action and by `refreshLayerTree` for orphaned entries. */
    stopStreamSource(layerId: number) {
        const src = this.streamSources.get(layerId);
        if (!src) return;
        src.stop();
        const next = new Map(this.streamSources);
        next.delete(layerId);
        this.streamSources = next;
    }

    /** React to a source ending *externally* (the browser's "Stop sharing" bar,
     *  a webcam unplug, or a Blender stream disconnect). Stop the source but
     *  keep it registered — the dead entry is the record of its own demise,
     *  feeding the error notice and status row in VoidProperties (pruning here
     *  would vanish the error before it could render). Drop the session opt-in
     *  so "Connect"/"Resume" shows again; the next start replaces the corpse.
     *  Pruning stays with `stopStreamSource` (delete / undo / orphan paths). */
    private onStreamSourceEnded(layerId: number) {
        this.streamSources.get(layerId)?.stop();
        this.streamSources = new Map(this.streamSources);
        this.clearStreamSessionStarted(layerId);
        // Timelapse "final" milestone: a feed dropping holds its last frame on
        // the GPU without any document change, so capture it before the render
        // loop settles. A no-op when recording is off.
        this.engine?.api.requestRecordingCapture();
        this.requestFrame();
    }

    /** Drop a layer's session opt-in (external end or failed acquisition), so
     *  VoidProperties re-shows the "Connect"/"Resume" affordance. */
    private clearStreamSessionStarted(layerId: number) {
        if (!this.streamSessionStarted.has(layerId)) return;
        const next = new Set(this.streamSessionStarted);
        next.delete(layerId);
        this.streamSessionStarted = next;
    }

    /** Surface a stream source's current state to the properties panel.
     *  Returns null when there's no source registered for the id (i.e. the
     *  layer isn't a stream-backed void or the source hasn't been created
     *  yet). */
    streamSourceFor(layerId: number): FrameSource | null {
        return this.streamSources.get(layerId) ?? null;
    }

    /** Reconcile the live `streamSources` map against the latest layer
     *  tree. Responsibilities: tear down sources whose void was deleted /
     *  undone, push the latest `freeze` + `frame_divisor` + effective-visibility
     *  into each live source, and prune the session-opt-in set. It does NOT
     *  start sources — that's a gesture-only concern (see `startMediaStreamVoid`)
     *  so activation never expires — and it does NOT stop a source merely
     *  because the void is frozen: freeze suppresses uploads while keeping the
     *  stream open (stopping a `getDisplayMedia` track would end the share for
     *  good). Called from `refreshLayerTree` after every layer mutation so dead
     *  streams are reaped and the OS capture indicator turns off when the layer
     *  actually goes away.
     *
     *  Takes the tree as a parameter (rather than reading `this.layerTree`) so
     *  the caller — `refreshLayerTree` — doesn't accidentally read the same
     *  reactive store it's about to write, which would loop Svelte's
     *  infinite-update guard. */
    private reconcileStreamSources(tree: any[]) {
        const desired = new Map<
            number,
            { frozen: boolean; frameDivisor: number; visible: boolean; url: string | null }
        >();
        // Thread `parentVisible` through the walk: a stream void is effectively
        // visible only if every ancestor up to the root is visible, matching
        // the compositor's nested-visibility semantics (see
        // `Doc::effective_visible`). The eye on the void's own row is necessary
        // but not sufficient — hiding the parent group must also halt uploads.
        const walk = (nodes: any[], parentVisible: boolean) => {
            for (const n of nodes) {
                const selfVisible = n?.visible !== false; // default true
                const effectiveVisible = parentVisible && selfVisible;
                // `type` (not `kind`) is the serde variant tag on `LayerInfo`
                // — set by `#[serde(tag = "type")]` in engine/types.rs. Any
                // void whose kind declares a `captureKind` is stream-backed.
                const cap = this.voidCaptureKind.get(n?.voidType);
                if (n?.type === 'void' && cap) {
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
                    // `url` only exists on `stream` voids (Blender); the
                    // reconciler pushes it into the `HttpStreamSource` so an edit
                    // in the properties panel reconnects. Absent for camera /
                    // screenshare, where it stays null and is never applied.
                    const urlParam = params.find((p) => p?.name === 'url');
                    const url =
                        typeof urlParam?.value === 'string'
                            ? urlParam.value
                            : typeof urlParam?.default === 'string'
                              ? urlParam.default
                              : null;
                    desired.set(n.id, { frozen, frameDivisor, visible: effectiveVisible, url });
                }
                if (Array.isArray(n?.children)) walk(n.children, effectiveVisible);
            }
        };
        walk(tree, true);

        // Tear down sources only for layers that actually disappeared (deleted
        // / undone). Freezing is handled by `setFrozen` below — it must keep
        // the stream open.
        for (const id of [...this.streamSources.keys()]) {
            if (!desired.has(id)) {
                this.stopStreamSource(id);
            }
        }

        // Push the latest `freeze`, `frame_divisor`, effective-visibility, and
        // upload resolution cap into every live source. Freeze suppresses
        // uploads (holding the last GPU frame) without closing the stream;
        // slider / eye-toggle / parent-hide / canvas-resize changes take effect
        // on the next rAF. For `stream` voids, a changed `url` reconnects the
        // `HttpStreamSource` (a no-op when unchanged).
        const maxSourceDimension = Math.max(this.docW, this.docH);
        for (const [id, { frozen, frameDivisor, visible, url }] of desired) {
            const src = this.streamSources.get(id);
            if (!src) continue;
            src.setFrozen(frozen);
            src.setFrameDivisor(frameDivisor);
            src.setVisible(visible);
            src.setMaxSourceDimension(maxSourceDimension);
            if (url !== null && src instanceof HttpStreamSource) {
                src.setUrl(url);
            }
        }

        // Drop session-started ids whose layer is gone so a future undo that
        // re-adds a different layer at the same id doesn't carry a stale opt-in.
        let pruned: Set<number> | null = null;
        for (const id of this.streamSessionStarted) {
            if (!desired.has(id)) {
                pruned ??= new Set(this.streamSessionStarted);
                pruned.delete(id);
            }
        }
        if (pruned) this.streamSessionStarted = pruned;
    }

    /** Remove a veil and keep `activeVeilIndex` consistent with the new list. */
    removeVeil(index: number) {
        if (!this.engine) return;
        this.engine.api.removeVeil({ index });
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
        if (!this.engine || from === to) return;
        this.engine.api.moveVeil({ from, to });
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
        this.foreground = { ...freshDocument.foreground };
        this.background = { ...freshDocument.background };
    }

    /** Sync the JS canvas-window mirror (`docW`/`docH`/`canvasOriginX`/
     *  `canvasOriginY`) from the engine's `canvas_rect()`. Call after any op
     *  that moves or resizes the canvas window (load, resize, crop) so the
     *  coordinate transforms in `coordinates.ts` recenter around the real
     *  window. Returns the `[ox, oy, w, h]` rect for callers that need it. */
    async syncCanvasRect(): Promise<[number, number, number, number] | null> {
        if (!this.engine) return null;
        const r = (await this.engine.api.canvasRect()) as {
            origin_x: number;
            origin_y: number;
            width: number;
            height: number;
        };
        this.canvasOriginX = r.origin_x;
        this.canvasOriginY = r.origin_y;
        this.docW = r.width;
        this.docH = r.height;
        return [r.origin_x, r.origin_y, r.width, r.height];
    }

    /** Re-read the layer tree and reconcile session state against it.
     *
     *  `adoptAppeared` makes rows that just came into existence the selection —
     *  passed by undo/redo so undoing a delete lands on the layer it brought
     *  back, matching both GIMP (which selects the restored layer) and Krita
     *  (which restores the pre-delete selection set). */
    async refreshLayerTree(opts?: { adoptAppeared?: boolean }): Promise<void> {
        if (!this.engine) return;
        const parsed = await this.engine.api.layerTree();
        const next: any[] = Array.isArray(parsed) ? parsed : [];
        // Stream-backed voids (camera / screenshare) own a MediaStream +
        // <video>; reconcile the live set against the new tree so deleted /
        // frozen / undone voids tear down (turning off the OS capture
        // indicator) and divisor/visibility changes propagate. Done BEFORE
        // assignment so this method only *writes* `layerTree` (never reads it),
        // keeping it out of any enclosing effect's dependency set — otherwise
        // the write loops back through it.
        this.reconcileStreamSources(next);
        this.reconcileSelection(next, opts?.adoptAppeared ?? false);
        this.layerTree = next;
        // Schedule a render frame: callers invoke this after layer mutations
        // (undo/redo, add/remove, drag/drop, etc.), and the engine may have
        // async work pending — dirty-pixel readbacks, content-bounds compute,
        // animation. Without a frame, drain_dirty_thumbnail_readbacks never
        // runs and the layer panel ends up showing pre-mutation thumbnails.
        this.requestFrame();
    }

    async refreshVeilList(): Promise<void> {
        if (!this.engine) return;
        const list = await this.engine.api.veilList();
        this.veilList = Array.isArray(list) ? list : [];
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

    /** Handle of the rAF scheduled by {@link requestFrame}, so {@link renderNow}
     *  can cancel a still-pending frame before rendering synchronously. */
    private _frameHandle = 0;

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

    /** True while a canvas pointer stroke/drag is in flight (any tool).
     *  Set by CanvasView's pointer dispatch. Generic, not brush-specific —
     *  it gates autosave so a snapshot never captures a half-committed
     *  stroke or runs its offscreen composite mid-stroke. */
    pointerActive = $state(false);

    /** Safe to take an autosave snapshot right now? False while the user
     *  is mid-stroke on the canvas or mid-drag in the brush builder. */
    get idleForSnapshot(): boolean {
        return !this.pointerActive && this._interactionCount === 0;
    }

    /** Zoom that frames the (possibly rotated) document inside the viewport.
     *  The rotated axis-aligned bounding box is what must fit, so a tilted
     *  canvas never clips its corners. `allowUpscale` lets a document smaller
     *  than the viewport enlarge past 1:1 to fill it (Fit to Screen); the
     *  default caps at 1:1, so Reset View never blows up a tiny document. */
    fitZoom(allowUpscale = false): number {
        const cos = Math.abs(Math.cos(this.rotation));
        const sin = Math.abs(Math.sin(this.rotation));
        const boxW = this.docW * cos + this.docH * sin;
        const boxH = this.docW * sin + this.docH * cos;
        const fit = Math.min(this.viewportW / boxW, this.viewportH / boxH);
        return allowUpscale ? fit : Math.min(fit, 1);
    }

    /** Reset rotation/mirror/pan and zoom-to-fit (Krita "Reset Display"). pan=0
     *  restores the document's default on-open framing. */
    resetView() {
        this.panX = 0;
        this.panY = 0;
        this.rotation = 0;
        this.mirrorH = false;
        this.zoom = this.fitZoom();
        this.requestFrame();
    }

    /** Frame the document in the viewport: zoom-to-fit (enlarging a document
     *  smaller than the viewport past 1:1 to fill it) and recenter, preserving
     *  the current rotation and mirror. The orientation-agnostic counterpart to
     *  {@link resetView} — GIMP "Fit Image in Window". */
    fitToScreen() {
        this.panX = 0;
        this.panY = 0;
        this.zoom = this.fitZoom(true);
        this.requestFrame();
    }

    /** Recenter the canvas in the viewport, leaving zoom, rotation, and mirror
     *  untouched — GIMP "Center Image in Window". */
    centerView() {
        this.panX = 0;
        this.panY = 0;
        this.requestFrame();
    }

    /** Schedule a render frame if one isn't already pending. */
    requestFrame() {
        if (this._framePending) return;
        this._framePending = true;
        this._frameHandle = requestAnimationFrame((ts) => {
            this._framePending = false;
            this.runFrame(ts / 1000.0);
        });
    }

    /** Drive one full frame synchronously in the current task — the synchronous
     *  counterpart to {@link requestFrame}. The canvas-resize path needs this:
     *  Firefox's zero-copy WebGPU present stalls the GPU process when a present
     *  straddles a swapchain reconfigure, so the just-enqueued `resize`
     *  (→ `surface.configure`) and the present must run in the same JS task as
     *  the `canvas.width`/`canvas.height` write, with no browser turn between. */
    renderNow(ts: number = performance.now() / 1000) {
        // A normal rAF may already be queued; running now makes it redundant.
        // Cancel it so we don't render twice and — critically — so `_framePending`
        // is not left set, which would make the next `requestFrame` a silent no-op.
        if (this._framePending) {
            cancelAnimationFrame(this._frameHandle);
            this._framePending = false;
        }
        this.runFrame(ts);
    }

    /** One frame's work: drain the request FIFO + composite (the single engine
     *  borrow), refresh the state mirror, run per-frame tool/pointer hooks, poll
     *  async readbacks, and self-schedule the animation loop. Shared by the
     *  deferred ({@link requestFrame}) and synchronous ({@link renderNow}) entry
     *  points; `ts` is in seconds. Must not touch `_framePending` — the rAF path
     *  owns clearing it at the seam it scheduled. */
    private runFrame(ts: number) {
        const engine = this.engine;
        if (!engine) return;
        // Push the latest external frames (webcam / screenshare / Blender
        // stream) into their void input textures BEFORE render — render
        // reads from those textures during composite, so a later upload
        // would lag by a frame.
        //
        // The frame count we pass to `tick` is the value the compositor's
        // master counter *will* hold once render increments it (inside
        // `update_animations`): one past the count the *previous* render
        // returned. Anticipating the increment keeps JS-side divisor gates
        // phase-locked with the Rust-side veil / overlay / void divisors
        // that check the post-increment value — so a camera `divisor=N`
        // fires on the same rAF as a veil `divisor=N`, not one off. (We
        // can't read `frame_count` directly anymore — it would be a third
        // competing engine borrow; render returns it on the state mirror.)
        const nextFrameCount = (this.engineState?.frameCount ?? 0) + 1;
        for (const src of this.streamSources.values()) {
            src.tick(nextFrameCount);
        }

        // The ONE engine borrow per frame: drains the request FIFO (which
        // resolves any pending `send`/`post` promises) then composites. A
        // re-entrant render reached via the event pump returns `busy` — the
        // outer render handles everything, so we bail without rescheduling.
        const frame = engine.render(ts);
        if (frame.busy) return;

        // Refresh the synchronously-readable engine-state mirror from
        // render's returned snapshot — no per-frame query; it's a downhill
        // projection of the borrow render already held this frame. This one
        // assignment updates everything the UI caches: frame/thumbnail
        // counters (thumbnail `$derived`s re-run when `thumbnailVersion`
        // changes) and document bools.
        if (frame.state) this.engineState = frame.state;

        // Per-frame tool hook — async state sync (e.g. GPU readback
        // completion). The instance's OWN tool runs against its OWN session,
        // so a background tab's frame drives its own tool, never the focused
        // one. Wrapped so a hook whose engine op was cancelled by a session
        // change mid-await settles cleanly (see tool_session.ts).
        void runHook(this.tool(this.activeToolId)?.onFrame?.());

        // Drain completed process-recording captures to the encoder
        // worker. No-op unless this tab's recorder is live.
        processRecording.pollFrame(this);

        // Pointer singletons tick with the focused canvas's frame — there is
        // one pointer, and these read/write the global `app` (the focused
        // instance). A background tab's frame must not drive them.
        if (getActiveInstance() === this) {
            // Global color-pick poll — drives both the color-picker tool and
            // the modifier-held `sampleColor` chord. Runs regardless of
            // active tool so a Ctrl-drag started in (e.g.) the brush tool
            // completes.
            pollPick();
            // Refresh the color-picker cursor against the latest foreground
            // committed by `pollPick`. Cheap when nothing changed.
            tickColorPickerCursor();
            // Refresh the clone set-source cursor — re-queries "needs source"
            // on brush change and shows/hides the crosshair. Cheap when
            // nothing changed (memo guards).
            tickCloneSourceCursor();
        }

        // Check for completed async copy/cut readback.
        if (this._copyCallback) {
            engine.api.pollCopyResult().then((result) => {
                if (result && this._copyCallback) {
                    const cb = this._copyCallback;
                    this._copyCallback = null;
                    cb(result);
                }
            });
        }

        // Check for completed async export readback.
        if (this._exportCallback) {
            engine
                .api.pollExportResult()
                .then((result) => {
                    if (result && this._exportCallback) {
                        const cb = this._exportCallback;
                        this._exportCallback = null;
                        cb({ width: result.width, height: result.height, rgba: result.bytes });
                    }
                });
        }

        // Check for completed async `.darkly` save readbacks. The bundle's
        // byte blobs arrive concatenated in `bytes`; slice them back out
        // into the per-blob shape `saveDocument.ts` expects.
        if (this._saveCallback) {
            engine.api.pollSaveResult().then((packed) => {
                if (!packed || !this._saveCallback) return;
                const cb = this._saveCallback;
                this._saveCallback = null;
                cb(unpackSaveBundle(packed));
            });
        }

        // Continue animation loop only when no UI interaction is
        // monopolizing the main thread.  One-shot renders (tool
        // actions, resize, etc.) always go through — only the
        // self-scheduling continuous loop is suppressed.
        const shouldContinue =
            frame.needsMore ||
            this._copyCallback ||
            this._exportCallback ||
            this._saveCallback;
        if (shouldContinue && this._interactionCount === 0) {
            this.requestFrame();
        }
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
 *  threading the dependency through).
 *
 *  Focus no longer touches sessions: each instance owns its own live session
 *  and tool state, so a focus switch simply changes which instance `app`
 *  resolves to. Every tab's tool state (transform floating, text edit, gradient
 *  placement) stays live and resumes exactly where it left off. */
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
