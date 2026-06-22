import type { Engine, EngineState } from '../engine/protocol';
import type { SaveBundle } from '../storage/saveDocument';
import { compute_view_matrices } from '../../wasm/pkg/darkly_wasm';
import { toolRegistry } from '../tools/registry';
import { pollPick } from '../tools/color_pick_sync';
import { tickColorPickerCursor } from '../tools/colorpicker_cursor';
import { CameraSource } from '../lib/cameraSource';

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
    readonly id: string =
        typeof crypto !== 'undefined' && 'randomUUID' in crypto
            ? crypto.randomUUID()
            : `instance-${Math.random().toString(36).slice(2)}`;

    engine = $state<Engine | null>(null);

    /** Stable key for this tab's crash-recovery snapshot. Distinct from
     *  `id` so it reads clearly at the recovery-store boundary; repeated
     *  autosaves overwrite one snapshot file per tab. A tab restored from
     *  a snapshot gets a fresh `recoveryId` (it's a new live tab). */
    readonly recoveryId: string =
        typeof crypto !== 'undefined' && 'randomUUID' in crypto
            ? crypto.randomUUID()
            : `recovery-${Math.random().toString(36).slice(2)}`;

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

    /** Registered destructive color-filter types (invert, …), fetched once
     *  at startup. Drives the dynamic, auto-discovered Colors-menu actions in
     *  `registerActions` — a new filter in the Rust core surfaces a menu
     *  entry with zero frontend edits. */
    filterTypes = $state<Array<{ type: string; displayName: string }>>([]);

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
    async loadRegistries(engine: Engine) {
        const buildMap = (
            arr: Array<{ type: string; displayName: string }>,
        ): Record<string, string> => {
            const m: Record<string, string> = {};
            for (const e of arr ?? []) m[e.type] = e.displayName;
            return m;
        };
        const [tools, veils, voids, blends, modifiers, layerKinds, filters] = await Promise.all([
            engine.send('tool_types'),
            engine.send('veil_types'),
            engine.send('void_types'),
            engine.send('blend_mode_types'),
            engine.send('modifier_types'),
            engine.send('layer_kind_types'),
            engine.send('filter_types'),
        ]);
        this.toolDisplayNames = buildMap(tools);
        this.veilDisplayNames = buildMap(veils);
        this.voidDisplayNames = buildMap(voids);
        this.blendModeDisplayNames = buildMap(blends);
        this.modifierDisplayNames = buildMap(modifiers);
        this.layerKindDisplayNames = buildMap(layerKinds);
        this.filterTypes = filters ?? [];
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
        engine.post('add_veil', { veil_type: type, params });
        if (visible === false) {
            // `veil_list` returns highest-index first, so the just-added veil
            // sits at index 0 of the array. The list send is enqueued after the
            // add above, so FIFO ordering guarantees it sees the new veil.
            const list = (await engine.send('veil_list')) as Array<{ index: number }>;
            const added = list[0];
            if (added) engine.post('set_veil_visible', { index: added.index, visible: false });
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

    // Active veil. Mutually exclusive with activeLayerId — the right
    // sidebar's properties pane shows the props of whichever is non-null.
    activeVeilIndex = $state<number | null>(null);

    // Session "isolate this node" flag. When set, the renderer shows only
    // that node's contribution (e.g. a mask renders grayscale on canvas).
    // Replaces the old per-layer `showMaskLayerId`.
    isolatedNodeId = $state<number | null>(null);

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
            this.engine?.post('set_isolated_node', { id: 0 });
            this.isolatedNodeId = null;
            this.requestFrame();
        }
        this.activeLayerId = id;
        this.selectedLayerIds = id === null ? new Set() : new Set([id]);
        this.activeVeilIndex = null;
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
        const order = this.flattenedVisibleIds();
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
            this.engine?.post('set_isolated_node', { id: 0 });
            this.isolatedNodeId = null;
            this.requestFrame();
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

    /** Walk the visible tree depth-first, returning every clickable node
     *  id in panel-top-to-panel-bottom order. Children of collapsed
     *  groups are skipped (the user can't see them, so shift-click
     *  shouldn't reach them). */
    private flattenedVisibleIds(): number[] {
        const out: number[] = [];
        const walk = (nodes: any[]) => {
            for (const n of nodes) {
                if (n?.id === undefined) continue;
                out.push(n.id);
                if (n.type === 'group' && !n.collapsed && Array.isArray(n.children)) {
                    walk(n.children);
                }
            }
        };
        walk(this.layerTree);
        return out;
    }

    /** Pick the first id in `set` that appears in panel order. Returns
     *  null when the set is empty or none of its ids are still in the
     *  tree. Caller is the active-layer demotion path for ctrl-click. */
    private firstInTreeOrder(set: Set<number>): number | null {
        if (set.size === 0) return null;
        for (const id of this.flattenedVisibleIds()) {
            if (set.has(id)) return id;
        }
        return null;
    }

    /** Reconcile `selectedLayerIds` and `activeLayerId` against the latest
     *  layer tree. Ids that no longer exist (deleted, undone, replaced by a
     *  bake result, etc.) drop out; if the active id disappeared, demote
     *  to the next remaining selected id in panel order or null. Called
     *  from `refreshLayerTree` so batch-delete / undo / cross-tab swap
     *  fallout is handled in one place.
     *
     *  Takes the tree as a parameter (rather than reading `this.layerTree`)
     *  so this code path stays write-only on `layerTree` — reading it here
     *  would tie the LayerPanel's `$effect` to the very state the method
     *  just wrote, looping Svelte's update guard. Same pattern as
     *  `reconcileCameraSources(next)`. */
    private pruneSelectionAgainstTree(tree: any[]) {
        const alive = new Set<number>();
        const visibleOrder: number[] = [];
        const walk = (nodes: any[]) => {
            for (const n of nodes) {
                if (n?.id === undefined) continue;
                alive.add(n.id);
                visibleOrder.push(n.id);
                if (Array.isArray(n.modifiers)) {
                    for (const m of n.modifiers) {
                        if (m?.id !== undefined) alive.add(m.id);
                    }
                }
                if (n.type === 'group' && !n.collapsed && Array.isArray(n.children)) {
                    walk(n.children);
                }
            }
        };
        walk(tree);

        let mutated = false;
        const nextSelected = new Set<number>();
        for (const id of this.selectedLayerIds) {
            if (alive.has(id)) nextSelected.add(id);
            else mutated = true;
        }
        if (mutated) this.selectedLayerIds = nextSelected;

        if (this.activeLayerId !== null && !alive.has(this.activeLayerId)) {
            let replacement: number | null = null;
            for (const id of visibleOrder) {
                if (nextSelected.has(id)) { replacement = id; break; }
            }
            this.activeLayerId = replacement;
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
        if (!this.engine) return;
        if (this.cameraSources.has(layerId)) return;
        const src = new CameraSource(layerId, this.engine);
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
        if (!this.engine) return;
        this.engine.post('remove_veil', { index });
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
        this.engine.post('move_veil', { from, to });
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

    /** Sync the JS canvas-window mirror (`docW`/`docH`/`canvasOriginX`/
     *  `canvasOriginY`) from the engine's `canvas_rect()`. Call after any op
     *  that moves or resizes the canvas window (load, resize, crop) so the
     *  coordinate transforms in `coordinates.ts` recenter around the real
     *  window. Returns the `[ox, oy, w, h]` rect for callers that need it. */
    async syncCanvasRect(): Promise<[number, number, number, number] | null> {
        if (!this.engine) return null;
        const r = (await this.engine.send('canvas_rect')) as {
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

    async refreshLayerTree(): Promise<void> {
        if (!this.engine) return;
        const parsed = await this.engine.send('layer_tree');
        const next: any[] = Array.isArray(parsed) ? parsed : [];
        // Camera voids own a MediaStream + <video>; reconcile the live set
        // against the new tree so freshly-added voids spin up, deleted /
        // frozen / undone voids tear down (turning off the OS camera
        // indicator). Done BEFORE assignment so this method only *writes*
        // `layerTree` (never reads it), keeping it out of any enclosing
        // effect's dependency set — otherwise the write loops back through it.
        this.reconcileCameraSources(next);
        this.pruneSelectionAgainstTree(next);
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
        const list = await this.engine.send('veil_list');
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

    /** Schedule a render frame if one isn't already pending. */
    requestFrame() {
        if (this._framePending) return;
        this._framePending = true;
        requestAnimationFrame((ts) => {
            this._framePending = false;
            const engine = this.engine;
            if (!engine) return;
            // Push the latest webcam / screenshare frames into their void
            // input textures BEFORE render — render reads from those textures
            // during composite, so a later upload would lag by a frame.
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
            for (const src of this.cameraSources.values()) {
                src.tick(nextFrameCount);
            }

            // The ONE engine borrow per frame: drains the request FIFO (which
            // resolves any pending `send`/`post` promises) then composites. A
            // re-entrant render reached via the event pump returns `busy` — the
            // outer render handles everything, so we bail without rescheduling.
            const frame = engine.render(ts / 1000.0);
            if (frame.busy) return;

            // Refresh the synchronously-readable engine-state mirror from
            // render's returned snapshot — no per-frame query; it's a downhill
            // projection of the borrow render already held this frame. This one
            // assignment updates everything the UI caches: frame/thumbnail
            // counters (thumbnail `$derived`s re-run when `thumbnailVersion`
            // changes) and document bools.
            if (frame.state) this.engineState = frame.state;

            // Per-frame tool hook — async state sync (e.g. GPU readback completion).
            toolRegistry.get(this.activeToolId)?.onFrame?.();

            // Global color-pick poll — drives both the color-picker tool and
            // the modifier-held `sampleColor` chord. Runs regardless of active
            // tool so a Ctrl-drag started in (e.g.) the brush tool completes.
            pollPick();

            // Refresh the color-picker cursor against the latest foreground
            // committed by `pollPick`. Cheap when nothing changed.
            tickColorPickerCursor();

            // Check for completed async copy/cut readback.
            if (this._copyCallback) {
                engine.send('poll_copy_result').then((result) => {
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
                    .send<{ width: number; height: number; bytes: Uint8Array }>('poll_export_result')
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
                engine.send<PackedSaveResult>('poll_save_result').then((packed) => {
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
