import { actions, sites } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { settings } from '../state/settings.svelte';
import { newDocument } from '../state/newDocument.svelte';
import { resizeCanvas } from '../state/resizeCanvas.svelte';
import { imageRescale } from '../state/imageRescale.svelte';
import { selectionModify } from '../state/selectionModify.svelte';
import { filterModal } from '../state/filterModal.svelte';
import { layerPicker } from '../state/layerPicker.svelte';
import type { ParamInfo } from '../ui/filters/filterParams';
import { exportTimelapse } from '../state/exportTimelapse.svelte';
import { loadError, parseLoadErrorMessage } from '../state/loadError.svelte';
import { toast } from '../state/toast.svelte';
import { toolRegistry } from '../tools/registry';
import { brushGraph } from '../state/brush_graph.svelte';
import { brushSession } from '../tools/brush.svelte';
import { registerBrushParamActions } from './brush_params';
import { registerSampleColorAction } from './sample_color';
import { registerCloneSourceAction } from './clone_source_gesture';
import { registerClipboardActions } from './clipboard';
import { registerPackActions } from './pack_actions';
import { pickOpenFile, type OpenedFile } from '../storage/fileHandle';
import { detectKind, isImageKind, type FileKind } from '../storage/detectKind';
import { decodeToRgba, placeSmartObjectFromBlob } from './place_smart_object';
import { saveDocument } from '../storage/saveDocument';
import { fontLibrary } from '../state/font_library.svelte';
import { processRecording } from '../recording/recorder.svelte';
import { shell } from '../multi_tab/shell.svelte';
import { about } from '../state/about.svelte';
import { commandPalette } from '../state/commandPalette.svelte';
import { openCheatsheet } from '../ui/cheatsheet';
import { links, openExternal } from '../links';

/** The commands that add something to the layer stack, in the order the
 *  layer panel's new-layer dropdown lists them. The dropdown renders straight
 *  from these registrations, so a new layer kind needs an action and nothing
 *  else: its label, icon and behaviour come along for free. */
export const NEW_LAYER_ACTION_IDS = [
    'newLayer',
    'newFilterLayer',
    'newVeil',
    'newVoid',
    'newGroup',
];

/** Walk the layer tree to find a node by id. The layer tree is the
 *  JSON shape produced by `app.refreshLayerTree`, with `children` on
 *  groups and `modifiers` on hosts. */
function findNodeInTree(nodes: any[], id: number): any | null {
    for (const n of nodes) {
        if (n.id === id) return n;
        if (n.children) {
            const found = findNodeInTree(n.children, id);
            if (found) return found;
        }
    }
    return null;
}

/** Strip the file extension from a picker-supplied name so we can use
 *  it as a tab title. Matches the basename-only convention already used
 *  by Save As (which seeds `set_document_name` from the chosen filename
 *  minus `.darkly`). */
function tabNameFromFile(fileName: string): string {
    const stripped = fileName.replace(/\.[^./]+$/, '');
    return stripped || 'Untitled';
}

/** Unified Open. Pick any supported file, sniff its kind, and route to
 *  the matching loader. Every Open lands in a new tab; image-as-layer
 *  in the current doc is the drag-drop gesture (`CanvasView`'s drop
 *  handler) or the clipboard paste, not this action.
 *
 *  Exported so the canvas drop handler can re-enter this flow when the
 *  artist drags a `.darkly` (drop bypasses the picker but routes to the
 *  same loader). */
export async function openFlow(): Promise<void> {
    const picked = await pickOpenFile();
    if (!picked) return;
    await routePickedFile(picked);
}

/** Dispatch a picked / dropped file to the right loader. Centralises
 *  the magic-byte sniff so the picker and the drop handler share one
 *  branch table. */
async function routePickedFile(picked: OpenedFile): Promise<void> {
    const kind = detectKind(picked.bytes);
    if (kind === 'darkly') {
        openDarklyAsTab(picked);
        return;
    }
    if (isImageKind(kind)) {
        await openImageAsTab(picked, kind);
        return;
    }
    toast.show('error', `Unsupported file type: ${picked.name}`);
}

/** Open a `.darkly` archive in a new tab. The engine's
 *  `open_document(bytes)` is all-or-nothing: a refused load is
 *  surfaced through `LoadErrorToast` and the failed tab is rolled
 *  back so the artist is left with their previous focus. Exposed so
 *  the canvas drop handler can route a dropped `.darkly` through the
 *  same path the picker uses. */
export function openDarklyAsTab(picked: OpenedFile): void {
    // Per the plan: opens land in a new tab so the previously-active
    // doc + its undo stack are untouched. Tab name reflects the file
    // name (the engine's `set_document_name` is overwritten by the
    // loaded manifest below; the shell's pendingName is just the
    // initial display before handle bootstrap finishes).
    const inst = shell.open(tabNameFromFile(picked.name));
    inst.fileHandle = picked.handle;
    // Seed the tab's recording scratch from the file's embedded recording
    // now, while the async engine bootstrap runs; the scratch lock orders
    // this ahead of the recorder's segment scan, so this session's capture
    // appends after the absorbed segments.
    void processRecording.absorbDarkly(inst, picked.bytes);
    inst.onHandleReady = async (engine) => {
        try {
            await engine.api.openDocument(picked.bytes);
            // Fonts embedded in the opened file join the personal library so
            // they survive reload and reach future tabs (the engine already
            // registered them into this handle during the load).
            void fontLibrary.absorbDarkly(picked.bytes);
            // Tab strip reads through the engine's `document_name`
            // request (which the loader populated from `manifest.name`),
            // but the shell's `nameVersion` doesn't bump on its own:
            // nudge it so the strip re-derives.
            const name = await engine.api.documentName();
            shell.setName(inst.id, name);
            // The loaded manifest's dimensions override whatever the tab
            // was seeded with; refresh the JS mirror so coord transforms
            // recenter around the real canvas size.
            // Sync the full canvas window (dims + plane origin): a loaded
            // `.darkly` may carry a non-zero `canvas_origin` from a crop.
            await inst.syncCanvasRect();
            await app.refreshLayerTree();
            await app.refreshVeilList();
            app.requestFrame();
        } catch (e) {
            loadError.show(parseLoadErrorMessage(e));
            shell.close(inst.id);
        }
    };
}

/** Open a PNG / JPEG / WebP in a new tab sized to the image's
 *  intrinsic dimensions, with the image as the single raster layer.
 *  No file handle is cached on the new tab; re-saving the image as
 *  `.darkly` is a Save As, not a write-back to the source PNG. */
async function openImageAsTab(picked: OpenedFile, kind: FileKind): Promise<void> {
    // BlobPart requires Uint8Array<ArrayBuffer>; TS 5.7+ defaults to
    // <ArrayBufferLike>. WASM-sourced bytes are non-shared.
    const decoded = await decodeToRgba(new Blob([picked.bytes as Uint8Array<ArrayBuffer>]));
    if (!decoded) {
        toast.show('error', `Failed to decode ${kind.toUpperCase()}: ${picked.name}`);
        return;
    }
    const { width, height, rgba } = decoded;

    const inst = shell.open(tabNameFromFile(picked.name), { width, height });
    inst.onHandleReady = async (engine) => {
        // Pass anchor = -1 (no specific layer): the new tab has no
        // bg seed (the `onHandleReady` presence suppresses it), so
        // paste lands at the bottom of root, which is the only sensible
        // position for the doc's first layer.
        await engine.api.pasteImage({ width, height, offset_x: 0, offset_y: 0, active_layer_id: -1 }, rgba);
        await app.refreshLayerTree();
        app.requestFrame();
    };
}

/** Decode an image file and paste it as a new raster layer in the
 *  CURRENT document. Used by the canvas drag-drop handler: drop is
 *  the explicit "artist wants this image in this doc" gesture (Open from
 *  the menu / Ctrl+O always lands a new tab instead).
 *
 *  Returns the new layer id, or `-1` on decode failure. */
export async function pasteImageIntoCurrent(file: File): Promise<number> {
    const engine = app.engine;
    if (!engine) return -1;
    const decoded = await decodeToRgba(file);
    if (!decoded) {
        toast.show('error', `Failed to decode dropped image: ${file.name}`);
        return -1;
    }
    const { id: layerId } = await engine.api.pasteImage({
            width: decoded.width,
            height: decoded.height,
            offset_x: 0,
            offset_y: 0,
            active_layer_id: app.activeLayerId ?? -1,
        },
        decoded.rgba,
    );
    app.selectLayer(layerId);
    await app.refreshLayerTree();
    app.requestFrame();
    return layerId;
}

/** Route a dropped file (from `CanvasView`'s `ondrop` handler) through
 *  the same kind-sniff the Open action uses:
 *    - `.darkly` → open as a new tab (mirrors Ctrl+O on a `.darkly`).
 *    - image → paste into the current tab as a raster layer, or, with Alt
 *      held, place it as a smart object.
 *    - anything else → toast, no-op.
 *
 *  Drag-drop deliberately diverges from the picker for images: a drop
 *  onto the canvas is the explicit "I want this here" gesture, while
 *  the Open action is the explicit "open as a document" gesture. Holding
 *  Alt selects the smart-object form rather than prompting: a chooser would
 *  charge every drop an extra click to serve the rarer case, and the two
 *  menu actions are the discoverable route. */
export async function handleDroppedFile(file: File, altKey = false): Promise<void> {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const kind = detectKind(bytes);
    if (kind === 'darkly') {
        openDarklyAsTab({ bytes, name: file.name, handle: null });
        return;
    }
    if (isImageKind(kind)) {
        if (altKey) await placeSmartObjectFromBlob(file, file.name);
        else await pasteImageIntoCurrent(file);
        return;
    }
    toast.show('error', `Unsupported file type: ${file.name}`);
}

export function registerActions() {
    // -- Binding sites --
    sites.register({ name: 'keyboard',   provides: ['layerId'], displayName: 'Anywhere' });
    sites.register({ name: 'layerEye',   provides: ['layerId'], displayName: 'Layer Eye' });
    sites.register({ name: 'layerThumb', provides: ['layerId'], displayName: 'Layer Thumbnail' });
    sites.register({ name: 'maskThumb',  provides: ['layerId', 'maskIndex', 'maskId'], displayName: 'Mask Thumbnail' });
    sites.register({ name: 'canvas',     provides: ['x', 'y'], displayName: 'Canvas' });
    sites.register({ name: 'layerPanel', provides: ['layerId'], displayName: 'Layer Panel' });

    // -- Edit --
    actions.register({
        id: 'undo',
        menuPath: ['Edit:10'],
        // The layer-tree refresh goes first: it diffs the tree against the
        // pre-undo shape to find what the operation restored, and any await in
        // between could let an unrelated refresh consume that difference.
        handler: async () => {
            app.engine?.api.undo();
            await app.refreshLayerTree({ adoptAppeared: true });
            await app.syncCanvasRect();
        },
    });
    actions.register({
        id: 'redo',
        menuPath: ['Edit:20'],
        handler: async () => {
            app.engine?.api.redo();
            await app.refreshLayerTree({ adoptAppeared: true });
            await app.syncCanvasRect();
        },
    });

    // -- Colors --
    actions.register({
        id: 'resetColors',
        menuPath: ['Colors:20'],
        handler: () => app.resetColors(),
    });
    actions.register({
        id: 'swapColors',
        menuPath: ['Colors:10'],
        handler: () => app.swapColors(),
    });

    // -- Selection --
    actions.register({
        id: 'selectAll',
        menuPath: ['Select:10'],
        handler: () => app.engine?.api.selectAll(),
    });
    actions.register({
        id: 'clearSelection',
        menuPath: ['Select:20'],
        handler: () => app.engine?.api.clearSelection(),
    });
    actions.register({
        id: 'clearSelectionContents',
        menuPath: ['Select:40'],
        handler: () => {
            if (app.activeLayerId != null) {
                app.engine?.api.clearSelectionContents({ id: app.activeLayerId });
            }
        },
    });
    actions.register({
        id: 'invertSelection',
        menuPath: ['Select:30'],
        handler: () => app.engine?.api.invertSelection(),
    });
    actions.register({
        id: 'maskToSelection',
        menuPath: ['Select:35'],
        // The engine op takes the mask filter's id, not the host's. Both
        // call sites (the mask context menu and the maskThumb $mod+click
        // gesture) pass it under `maskId`; the enabled-guard fallback
        // resolves it from the active node when dispatched keyboard-only.
        enabled: () => app.activeMaskId != null || 'No mask on the active layer',
        handler: (ctx) => {
            const engine = app.engine;
            const maskId = ctx.maskId ?? app.activeMaskId;
            if (!engine || maskId == null) return;
            engine.api.maskToSelection({ id: maskId });
            app.requestFrame();
        },
    });
    actions.register({
        id: 'alphaToSelection',
        menuPath: ['Select:36'],
        // Sibling of `maskToSelection`, for the host rather than its mask.
        // The engine op is node-kind agnostic (it reads whatever texture the
        // id resolves to), but only pixel-bearing nodes have one, so the
        // guard follows the same fact the layer panel uses to decide whether
        // to draw a thumbnail at all.
        enabled: () => app.activeNode?.hasThumbnail === true || 'Active layer has no pixels',
        handler: (ctx) => {
            const engine = app.engine;
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (!engine || layerId == null) return;
            engine.api.alphaToSelection({ id: layerId });
            app.requestFrame();
        },
    });
    actions.register({
        id: 'growSelection',
        menuPath: ['Select:50'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('grow'),
    });
    actions.register({
        id: 'shrinkSelection',
        menuPath: ['Select:60'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('shrink'),
    });
    actions.register({
        id: 'borderSelection',
        menuPath: ['Select:70'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('border'),
    });
    actions.register({
        id: 'smoothSelection',
        menuPath: ['Select:80'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => app.engine?.api.smoothSelection({ radius: 2 }),
    });
    actions.register({
        id: 'featherSelection',
        menuPath: ['Select:90'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('feather'),
    });
    actions.register({
        id: 'antialiasSelection',
        menuPath: ['Select:100'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => app.engine?.api.antialiasSelection(),
    });

    // -- Image (canvas) --
    actions.register({
        id: 'resizeCanvas',
        menuPath: ['Image:10'],
        handler: () => {
            if (!app.engine) return;
            resizeCanvas.open = true;
        },
    });
    actions.register({
        id: 'rescaleImage',
        menuPath: ['Image:11'],
        handler: () => {
            if (!app.engine) return;
            imageRescale.open = true;
        },
    });
    actions.register({
        id: 'cropToSelection',
        menuPath: ['Image:20'],
        // `enabled` is synchronous and `has_selection` is async, so we gate on
        // the `engineState` mirror (refreshed from render's snapshot) rather
        // than a live query.
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: async () => {
            app.engine?.api.cropToSelection();
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'flipCanvasH',
        menuPath: ['Image:30'],
        handler: async () => {
            app.engine?.api.flipCanvas({ axis: 'h' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'flipCanvasV',
        menuPath: ['Image:31'],
        handler: async () => {
            app.engine?.api.flipCanvas({ axis: 'v' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'rotateCanvasCW',
        menuPath: ['Image:40'],
        handler: async () => {
            app.engine?.api.rotateCanvas({ dir: 'cw' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'rotateCanvasCCW',
        menuPath: ['Image:41'],
        handler: async () => {
            app.engine?.api.rotateCanvas({ dir: 'ccw' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'rotateCanvas180',
        menuPath: ['Image:42'],
        handler: async () => {
            app.engine?.api.rotateCanvas({ dir: '180' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });

    // -- Clipboard --
    registerClipboardActions();

    // -- File I/O --
    actions.register({
        id: 'saveDocument',
        menuPath: ['File:30'],
        handler: () => {
            if (!app.engine) return;
            void saveDocument({ forceAs: false });
        },
    });
    actions.register({
        id: 'saveDocumentAs',
        menuPath: ['File:40'],
        handler: () => {
            if (!app.engine) return;
            void saveDocument({ forceAs: true });
        },
    });
    actions.register({
        id: 'newDocument',
        menuPath: ['File:10'],
        // No default hotkey: `$mod+KeyN` is reserved by every major browser
        // for "new window" and cannot be intercepted by the page. Artists can
        // still bind it via the Hotkeys tab if their browser/OS allows.
        handler: () => {
            newDocument.open = true;
        },
    });
    actions.register({
        id: 'open',
        menuPath: ['File:20'],
        handler: () => {
            void openFlow();
        },
    });
    actions.register({
        id: 'placeSmartObject',
        menuPath: ['File:21'],
        handler: () => {
            void (async () => {
                if (!app.engine) return;
                const picked = await pickOpenFile();
                if (!picked) return;
                const kind = detectKind(picked.bytes);
                if (!isImageKind(kind)) {
                    toast.show('error', `Not an image: ${picked.name}`);
                    return;
                }
                await placeSmartObjectFromBlob(
                    new Blob([picked.bytes as Uint8Array<ArrayBuffer>]),
                    picked.name,
                );
            })();
        },
    });
    actions.register({
        id: 'exportTimelapse',
        menuPath: ['File:51'],
        handler: () => {
            if (!app.engine) return;
            exportTimelapse.open = true;
        },
    });
    // -- Floating content / transform --
    actions.register({
        id: 'commitFloating',
        handler: () => {
            if (!app.engine) return;
            app.engine.api.commitFloating();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'cancelFloating',
        handler: () => {
            if (!app.engine) return;
            app.engine.api.cancelFloating();
            app.requestFrame();
        },
    });

    // -- Tools (generated from registry) --
    // Tool key bindings come from the YAML preset layers (defaults.yaml +
    // overlay) via `hotkeys.<hotkeyAction>`: actions register without any
    // built-in default; the binding is purely configuration.
    //
    // A tool-selecting action's documentation is the tool's own: the id, label,
    // glyph and summary all live on its `ToolRegistration` and reach here
    // through the `tools` catalog, so it passes an explicit `doc` rather than
    // resolving through the `actions` catalog. Only the "Switch to …" phrasing
    // and the erase-mode glyph override are this side's.
    for (const tool of toolRegistry.all()) {
        // A descriptor the core has no registration for would select a tool that
        // does not exist, so it gets no action.
        const entry = app.entry('tools', tool.id);
        if (!entry?.hotkeyAction) continue;
        const name = entry.displayName;
        actions.register({
            id: entry.hotkeyAction,
            doc: {
                displayName: name,
                category: 'tools',
                description: `Switch to ${name} tool`,
                icon: app.toolGlyph(tool.id),
            },
            handler: () => { app.activeToolId = tool.id; },
        });
    }

    // Erase mode is a flag on the brush tool, not a tool of its own.
    // Hitting the hotkey from any other tool flips to brush AND turns
    // erase on (matches Krita's "E from anywhere paints with the eraser").
    actions.register({
        id: 'toggleEraseMode',
        status: () => (brushSession.eraseMode ? 'fa6-solid:check' : undefined),
        handler: () => {
            if (app.activeToolId !== 'brush') {
                app.activeToolId = 'brush';
            }
            // No-op when the active brush's terminal opts out of erase
            // (smudge, liquify, watercolor). Same reason the BrushOptions
            // toggle is hidden: flipping `gpu.blend_mode` would do
            // nothing, so the hotkey should match the visible UI.
            if (!brushGraph.supportsErase) {
                return;
            }
            brushSession.eraseMode = !brushSession.eraseMode;
            app.engine?.api.setBrushBlendMode({ mode: brushSession.eraseMode ? 1 : 0 });
        },
    });

    // -- Layers --
    actions.register({
        id: 'newLayer',
        menuPath: ['Layer:10'],
        handler: async () => {
            const engine = app.engine;
            if (!engine) return;
            const id = await engine.api.addRaster({ anchor: app.activeLayerId });
            app.selectLayer(id);
            await app.refreshLayerTree();
        },
    });

    actions.register({
        id: 'newFilterLayer',
        menuPath: ['Layer:12'],
        handler: () => { layerPicker.kind = 'filter'; },
    });

    actions.register({
        id: 'newVeil',
        menuPath: ['Layer:14'],
        handler: () => { layerPicker.kind = 'veil'; },
    });

    actions.register({
        id: 'newVoid',
        menuPath: ['Layer:16'],
        handler: () => { layerPicker.kind = 'void'; },
    });

    actions.register({
        id: 'newGroup',
        menuPath: ['Layer:20'],
        handler: async () => {
            const engine = app.engine;
            if (!engine) return;
            if (app.selectedLayerIds.size > 0) {
                try {
                    const groupId = await engine.api.groupLayers({
                        ids: [...app.selectedLayerIds],
                    });
                    if (groupId) app.selectLayer(groupId);
                } catch (e: any) {
                    toast.show('error', e.message ?? String(e));
                }
            } else {
                const id = await engine.api.addGroup({ anchor: app.activeLayerId });
                app.selectLayer(id);
            }
            await app.refreshLayerTree();
        },
    });

    actions.register({
        id: 'toggleVisibility',
        menuPath: ['Layer:70'],
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.engine) return;
            const layer = findLayer(app.layerTree, layerId);
            if (layer) {
                app.engine.api.setLayerVisible({ id: layerId, visible: !layer.visible });
            }
        },
    });

    actions.register({
        id: 'toggleLock',
        menuPath: ['Layer:80'],
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.engine) return;
            const layer = findLayer(app.layerTree, layerId);
            if (layer) {
                app.engine.api.setNodeLocked({ id: layerId, locked: !layer.locked });
            }
        },
    });

    actions.register({
        id: 'isolateLayer',
        menuPath: ['Layer:90'],
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.engine) return;
            toggleIsolation(layerId);
        },
    });

    actions.register({
        id: 'deleteLayer',
        menuPath: ['Layer:60'],
        handler: async () => {
            const engine = app.engine;
            if (!engine) return;
            // Veil takes priority: the trash button on the layer panel
            // doubles as veil-remove when a veil is active, so the
            // keyboard shortcut should too.
            if (app.activeVeilIndex !== null) {
                app.removeVeil(app.activeVeilIndex);
                return;
            }
            // Structural rule: operate on the current selection. The
            // right-click handler ensures the clicked row is in the
            // selection BEFORE the menu opens, so reading from
            // `app.selectedLayerIds` here picks up exactly what the artist
            // expects. We do NOT accept a `ctx.layerId` override: the
            // v1 attempt did, and that's what made "Delete N Layers" act
            // on just one layer.
            const targets = app.selectedLayerIds.size > 0
                ? [...app.selectedLayerIds]
                : app.activeLayerId !== null ? [app.activeLayerId] : [];
            if (targets.length === 0) return;
            try {
                // Stop any associated frame sources (camera / screenshare /
                // Blender stream) before the layers go away. `refreshLayerTree`
                // reaps as a safety net, but stopping eagerly turns off the OS
                // capture indicator / drops the HTTP connection immediately.
                for (const id of targets) app.stopStreamSource(id);
                if (targets.length === 1) {
                    await engine.api.removeLayer({ id: targets[0] });
                } else {
                    const skipped = await engine.api.removeLayers({ ids: targets });
                    if (skipped > 0) {
                        toast.show('info', `${skipped} locked layer${skipped === 1 ? '' : 's'} skipped`);
                    }
                }
                await app.refreshLayerTree();
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        },
    });

    actions.register({
        id: 'duplicateLayer',
        menuPath: ['Layer:30'],
        handler: async () => {
            const engine = app.engine;
            if (!engine) return;
            const targets = app.selectedLayerIds.size > 0
                ? [...app.selectedLayerIds]
                : app.activeLayerId !== null ? [app.activeLayerId] : [];
            if (targets.length === 0) return;
            if (targets.length === 1) {
                const newId = await engine.api.duplicateNode({ source_id: targets[0] });
                await app.refreshLayerTree();
                if (newId) app.selectLayer(newId);
            } else {
                const newIds = await engine.api.duplicateNodes({ ids: targets });
                await app.refreshLayerTree();
                if (newIds.length > 0) app.selectLayers(newIds);
            }
        },
    });

    actions.register({
        id: 'flipLayerH',
        menuPath: ['Layer:40'],
        enabled: () => app.activeLayerId !== null || 'No active layer',
        handler: async () => {
            const engine = app.engine;
            if (!engine || app.activeLayerId === null) return;
            await engine.api.flipNode({ node_id: app.activeLayerId, xform: 'flip_h' });
            app.requestFrame();
        },
    });
    actions.register({
        id: 'flipLayerV',
        menuPath: ['Layer:50'],
        enabled: () => app.activeLayerId !== null || 'No active layer',
        handler: async () => {
            const engine = app.engine;
            if (!engine || app.activeLayerId === null) return;
            await engine.api.flipNode({ node_id: app.activeLayerId, xform: 'flip_v' });
            app.requestFrame();
        },
    });
    // Destructive color filters (invert, …) are registered dynamically
    // from the Rust filter-pipeline registry (the `filters` catalog fetched
    // during `loadRegistries`), so a new filter in the core surfaces a
    // Colors-menu entry with no frontend edit. The target is the active *node*
    // (`activeLayerId` is the mask filter id when a mask is selected), which
    // is what makes "invert the mask" reachable from the same entry.
    for (const flt of app.entries?.('filters') ?? []) {
        const filterType = flt.type;
        if (!flt.hotkeyAction) continue;
        // A parametric filter (curves/levels/hsv) can't apply in one click: its
        // params must be authored first, so it opens the modal (the same
        // `FilterParamsEditor` the layer panel uses). Param-free filters (invert)
        // apply immediately.
        const parametric = (flt.params?.length ?? 0) > 0;
        actions.register({
            id: flt.hotkeyAction,
            // Like tool selection, the documentation is the filter's own and
            // arrives through the `filters` catalog. What this side composes is
            // the phrasing: the `…` that marks a filter as opening a dialog,
            // and the note about what the filter lands on.
            doc: {
                displayName: parametric ? `${flt.displayName}…` : flt.displayName,
                category: 'layers',
                // Lead with the registry's own summary: the command palette's
                // substring search indexes descriptions, so its keywords (e.g.
                // "desaturate" for Black and White) keep the filter findable.
                description: `${flt.description ?? ''} Applies to the active layer or mask (respecting any selection).`.trim(),
                icon: flt.icon ?? '',
            },
            menuPath: ['Colors:10'],
            enabled: () => app.activeLayerId !== null || 'No active layer',
            handler: async () => {
                const engine = app.engine;
                if (!engine || app.activeLayerId === null) return;
                if (parametric) {
                    filterModal.show(
                        app.activeLayerId,
                        filterType,
                        flt.displayName,
                        (flt.params ?? []) as unknown as ParamInfo[]
                    );
                    return;
                }
                await engine.api.applyFilter({
                    node_id: app.activeLayerId,
                    filter_type: filterType,
                    params: {},
                });
                app.requestFrame();
            },
        });
    }

    actions.register({
        id: 'mergeDown',
        menuPath: ['Layer:110'],
        handler: async () => {
            const engine = app.engine;
            if (!engine) return;
            if (app.selectedLayerIds.size >= 2) {
                try {
                    const newId = await engine.api.mergeLayers({
                        ids: [...app.selectedLayerIds],
                    });
                    await app.refreshLayerTree();
                    if (newId) app.selectLayer(newId);
                } catch (e: any) {
                    toast.show('error', e.message ?? String(e));
                }
                return;
            }
            const sourceId = app.activeLayerId;
            if (sourceId == null) return;
            try {
                const newId = await engine.api.mergeDown({ source_id: sourceId });
                await app.refreshLayerTree();
                if (newId) app.selectLayer(newId);
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        },
    });

    actions.register({
        id: 'flatten',
        menuPath: ['Layer:120'],
        handler: async (ctx) => {
            const engine = app.engine;
            if (!engine) return;
            const id = ctx.layerId ?? app.activeLayerId;
            if (id == null) return;
            try {
                const newId = await engine.api.flattenNode({ node_id: id });
                await app.refreshLayerTree();
                if (newId) app.selectLayer(newId);
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        },
    });

    actions.register({
        id: 'addMask',
        menuPath: ['Layer:100'],
        handler: async (ctx) => {
            const engine = app.engine;
            if (!engine) return;
            const hostId = ctx.layerId ?? app.activeLayerId;
            if (hostId == null) return;
            engine.api.addMask({ id: hostId });
            // `add_mask` doesn't return the new modifier id, and we want
            // the mask to be the active paint target after creation:
            // refresh the tree, then locate the freshly-added mask
            // modifier on the host and select it.
            await app.refreshLayerTree();
            const layer = findNodeInTree(app.layerTree, hostId);
            const mask = layer?.modifiers?.find((m: any) => m.kind === 'mask');
            if (mask) app.selectLayer(mask.id);
        },
    });

    // -- View --
    actions.register({
        id: 'openSettings',
        // No `menuPath`: surfaced as the gear button on the menu bar and a
        // root courtesy item in the hamburger, not as a View submenu row.
        handler: () => { settings.open = true; },
    });

    actions.register({
        id: 'mirrorViewH',
        menuPath: ['View:10'],
        status: () => (app.mirrorH ? 'fa6-solid:check' : undefined),
        handler: () => {
            app.mirrorH = !app.mirrorH;
            app.requestFrame();
        },
    });

    actions.register({
        id: 'resetView',
        menuPath: ['View:11'],
        handler: () => { app.resetView(); },
    });

    actions.register({
        id: 'fitToScreen',
        menuPath: ['View:12'],
        handler: () => { app.fitToScreen(); },
    });

    actions.register({
        id: 'centerView',
        menuPath: ['View:13'],
        handler: () => { app.centerView(); },
    });

    actions.register({
        id: 'commandPalette',
        // No `menuPath`: surfaced as the prominent "Find" item at the top of
        // the hamburger / on the menu bar, not as a buried submenu row.
        handler: () => { commandPalette.open = true; },
    });

    actions.register({
        id: 'openCheatsheet',
        menuPath: ['Help:10'],
        handler: () => openCheatsheet(),
    });

    actions.register({
        id: 'openDocs',
        menuPath: ['Help:20'],
        handler: () => openExternal(links.docs),
    });

    actions.register({
        id: 'openWebsite',
        menuPath: ['Help:30'],
        handler: () => openExternal(links.website),
    });

    actions.register({
        id: 'openGithub',
        menuPath: ['Help:40'],
        handler: () => openExternal(links.github),
    });

    actions.register({
        id: 'aboutDarkly',
        menuPath: ['Help:50'],
        handler: () => { about.open = true; },
    });

    // -- Brush parameters (size hotkeys + shift+drag scrub) --
    registerBrushParamActions();

    // -- Modifier-held color picker (Ctrl+drag → sample color) --
    registerSampleColorAction();

    // -- Clone brush set-source gesture (brush-scoped modifier+drag) --
    registerCloneSourceAction();

    // -- Brush pack import / export --
    registerPackActions();

    // -- Brush builder --
    actions.register({
        id: 'addBrushNode',
        handler: () => {
            // No-op if the brush builder isn't visible. The actual placement
            // (at the cursor in canvas coords) happens in NodeCanvas, which
            // owns pan/zoom and the cursor; we just signal it via an event.
            if (!brushGraph.isOpen) return;
            window.dispatchEvent(new CustomEvent('darkly:add-node-request'));
        },
    });
}

// -- Layer isolation --
//
// Isolation is pure session state: the engine's `isolated_node` is the
// single source of truth. We never touch `set_layer_visible` here, so eye
// icons stay independent: an artist can toggle visibility on hidden siblings
// while soloed and those changes persist after un-solo.

function toggleIsolation(targetId: number) {
    if (!app.engine) return;
    const next = app.isolatedNodeId === targetId ? null : targetId;
    void app.setIsolatedNode(next);
}

/** Find a layer by id in the tree (recursive search). */
function findLayer(tree: any[], id: number): any | undefined {
    for (const node of tree) {
        if (node.id === id) return node;
        if (node.children) {
            const found = findLayer(node.children, id);
            if (found) return found;
        }
    }
    return undefined;
}
