import { actions, sites } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { settings } from '../state/settings.svelte';
import { newDocument } from '../state/newDocument.svelte';
import { resizeCanvas } from '../state/resizeCanvas.svelte';
import { imageRescale } from '../state/imageRescale.svelte';
import { selectionModify } from '../state/selectionModify.svelte';
import { filterModal } from '../state/filterModal.svelte';
import type { FilterParam } from '../ui/filters/filterParams';
import { exportImage } from '../state/exportImage.svelte';
import { exportTimelapse } from '../state/exportTimelapse.svelte';
import { loadError, parseLoadErrorMessage } from '../state/loadError.svelte';
import { toast } from '../state/toast.svelte';
import { toolRegistry, type ToolDescriptor } from '../tools/registry';
import { brushGraph } from '../state/brush_graph.svelte';
import { brushSession } from '../tools/brush.svelte';
import { registerBrushParamActions } from './brush_params';
import { registerSampleColorAction } from './sample_color';
import { registerCloneSourceAction } from './clone_source_gesture';
import { registerClipboardActions } from './clipboard';
import { pickOpenFile, type OpenedFile } from '../storage/fileHandle';
import { detectKind, isImageKind, type FileKind } from '../storage/detectKind';
import { saveDocument } from '../storage/saveDocument';
import { fontLibrary } from '../state/font_library.svelte';
import { processRecording } from '../recording/recorder.svelte';
import { canSave } from '../storage/fileHandle';
import { shell } from '../multi_tab/shell.svelte';
import { about } from '../state/about.svelte';
import { commandPalette } from '../state/commandPalette.svelte';
import { openCheatsheet } from '../ui/cheatsheet';
import { links, openExternal } from '../links';

// Tooltip explaining why Save / Save As are disabled when the browser lacks
// the File System Access API (Firefox). Returned from each save action's
// `enabled()` as the disabled-reason string, surfaced as the menu row's `title`.
const NO_SAVE_TOOLTIP =
    "Filesystem save isn't supported in this browser — try Chrome, Edge, or Safari.";

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

/** The Iconify icon name for a tool-switch action — the tool's own `icon`,
 *  falling back to a generic glyph for the (now hypothetical) tool that ships
 *  none. Every tool currently declares one. */
function glyphFromTool(tool: ToolDescriptor): string {
    return tool.icon ?? 'fa6-solid:wrench';
}

/** Unified Open. Pick any supported file, sniff its kind, and route to
 *  the matching loader. Every Open lands in a new tab — image-as-layer
 *  in the current doc is the drag-drop gesture (`CanvasView`'s drop
 *  handler) or the clipboard paste, not this action.
 *
 *  Exported so the canvas drop handler can re-enter this flow when the
 *  user drags a `.darkly` (drop bypasses the picker but routes to the
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
 *  `open_document(bytes)` is all-or-nothing — a refused load is
 *  surfaced through `LoadErrorToast` and the failed tab is rolled
 *  back so the user is left with their previous focus. Exposed so
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
    // now, while the async engine bootstrap runs — the scratch lock orders
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
            // but the shell's `nameVersion` doesn't bump on its own —
            // nudge it so the strip re-derives.
            const name = await engine.api.documentName();
            shell.setName(inst.id, name);
            // The loaded manifest's dimensions override whatever the tab
            // was seeded with; refresh the JS mirror so coord transforms
            // recenter around the real canvas size.
            // Sync the full canvas window (dims + plane origin) — a loaded
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
 *  No file handle is cached on the new tab — re-saving the image as
 *  `.darkly` is a Save As, not a write-back to the source PNG. */
async function openImageAsTab(picked: OpenedFile, kind: FileKind): Promise<void> {
    let bitmap: ImageBitmap;
    try {
        // BlobPart requires Uint8Array<ArrayBuffer>; TS 5.7+ defaults to
        // <ArrayBufferLike>. WASM-sourced bytes are non-shared.
        bitmap = await createImageBitmap(new Blob([picked.bytes as Uint8Array<ArrayBuffer>]));
    } catch (e) {
        toast.show('error', `Failed to decode ${kind.toUpperCase()}: ${picked.name}`);
        console.error('[open] image decode failed', e);
        return;
    }
    const { width, height } = bitmap;

    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext('2d');
    if (!ctx) {
        bitmap.close();
        toast.show('error', '2D canvas context unavailable');
        return;
    }
    ctx.drawImage(bitmap, 0, 0);
    bitmap.close();
    const rgba = new Uint8Array(ctx.getImageData(0, 0, width, height).data.buffer);

    const inst = shell.open(tabNameFromFile(picked.name), { width, height });
    inst.onHandleReady = async (engine) => {
        // Pass anchor = -1 (no specific layer) — the new tab has no
        // bg seed (the `onHandleReady` presence suppresses it), so
        // paste lands at the bottom of root, which is the only sensible
        // position for the doc's first layer.
        await engine.api.pasteImage({ width, height, offset_x: 0, offset_y: 0, active_layer_id: -1 }, rgba);
        await app.refreshLayerTree();
        app.requestFrame();
    };
}

/** Decode an image file and paste it as a new raster layer in the
 *  CURRENT document. Used by the canvas drag-drop handler — drop is
 *  the explicit "user wants this image in this doc" gesture (Open from
 *  the menu / Ctrl+O always lands a new tab instead).
 *
 *  Returns the new layer id, or `-1` on decode failure. */
export async function pasteImageIntoCurrent(file: File): Promise<number> {
    const engine = app.engine;
    if (!engine) return -1;
    try {
        const bitmap = await createImageBitmap(file);
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const ctx = canvas.getContext('2d')!;
        ctx.drawImage(bitmap, 0, 0);
        bitmap.close();
        const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);
        const rgba = new Uint8Array(imageData.data.buffer);
        const activeId = app.activeLayerId ?? -1;
        const { id: layerId } = await engine.api.pasteImage({
                width: canvas.width,
                height: canvas.height,
                offset_x: 0,
                offset_y: 0,
                active_layer_id: activeId,
            },
            rgba,
        );
        app.selectLayer(layerId);
        await app.refreshLayerTree();
        app.requestFrame();
        return layerId;
    } catch (e) {
        toast.show('error', `Failed to decode dropped image: ${file.name}`);
        console.error('[drop] image decode failed', e);
        return -1;
    }
}

/** Route a dropped file (from `CanvasView`'s `ondrop` handler) through
 *  the same kind-sniff the Open action uses:
 *    - `.darkly` → open as a new tab (mirrors Ctrl+O on a `.darkly`).
 *    - image → paste into the current tab as a raster layer.
 *    - anything else → toast, no-op.
 *
 *  Drag-drop deliberately diverges from the picker for images: a drop
 *  onto the canvas is the explicit "I want this here" gesture, while
 *  the Open action is the explicit "open as a document" gesture. */
export async function handleDroppedFile(file: File): Promise<void> {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const kind = detectKind(bytes);
    if (kind === 'darkly') {
        openDarklyAsTab({ bytes, name: file.name, handle: null });
        return;
    }
    if (isImageKind(kind)) {
        await pasteImageIntoCurrent(file);
        return;
    }
    toast.show('error', `Unsupported file type: ${file.name}`);
}

export function registerActions() {
    // -- Binding sites --
    sites.register({ name: 'keyboard',   provides: ['layerId'], displayName: 'Anywhere' });
    sites.register({ name: 'layerEye',   provides: ['layerId'], displayName: 'Layer Eye' });
    sites.register({ name: 'layerThumb', provides: ['layerId'], displayName: 'Layer Thumbnail' });
    sites.register({ name: 'maskThumb',  provides: ['layerId', 'maskIndex'], displayName: 'Mask Thumbnail' });
    sites.register({ name: 'canvas',     provides: ['x', 'y'], displayName: 'Canvas' });
    sites.register({ name: 'layerPanel', provides: ['layerId'], displayName: 'Layer Panel' });

    // -- Edit --
    actions.register({
        id: 'undo',
        displayName: 'Undo',
        category: 'edit',
        description: 'Undo the last action.',
        icon: 'fa6-solid:rotate-left',
        menuPath: ['Edit:10'],
        handler: async () => { app.engine?.api.undo(); await app.syncCanvasRect(); await app.refreshLayerTree(); },
    });
    actions.register({
        id: 'redo',
        displayName: 'Redo',
        category: 'edit',
        description: 'Redo the last undone action.',
        icon: 'fa6-solid:rotate-right',
        menuPath: ['Edit:20'],
        handler: async () => { app.engine?.api.redo(); await app.syncCanvasRect(); await app.refreshLayerTree(); },
    });

    // -- Colors --
    actions.register({
        id: 'resetColors',
        displayName: 'Reset Colors',
        category: 'colors',
        description: 'Reset the foreground/background to black and white.',
        icon: 'fa6-solid:circle-half-stroke',
        menuPath: ['Colors:20'],
        handler: () => app.resetColors(),
    });
    actions.register({
        id: 'swapColors',
        displayName: 'Swap Colors',
        category: 'colors',
        description: 'Swap the foreground and background colors.',
        icon: 'fa6-solid:right-left',
        menuPath: ['Colors:10'],
        handler: () => app.swapColors(),
    });

    // -- Selection --
    actions.register({
        id: 'selectAll',
        displayName: 'Select All',
        category: 'selection',
        description: 'Select the entire canvas.',
        icon: 'fa6-solid:vector-square',
        menuPath: ['Select:10'],
        handler: () => app.engine?.api.selectAll(),
    });
    actions.register({
        id: 'clearSelection',
        displayName: 'Deselect',
        category: 'selection',
        description: 'Clear the active selection.',
        icon: 'fa6-solid:ban',
        menuPath: ['Select:20'],
        handler: () => app.engine?.api.clearSelection(),
    });
    actions.register({
        id: 'clearSelectionContents',
        displayName: 'Clear Selection Contents',
        category: 'selection',
        description: 'Erase the pixels inside the selection.',
        icon: 'fa6-solid:eraser',
        menuPath: ['Select:40'],
        handler: () => {
            if (app.activeLayerId != null) {
                app.engine?.api.clearSelectionContents({ id: app.activeLayerId });
            }
        },
    });
    actions.register({
        id: 'invertSelection',
        displayName: 'Invert Selection',
        category: 'selection',
        description: 'Invert the current selection.',
        icon: 'tabler:flip-horizontal',
        menuPath: ['Select:30'],
        handler: () => app.engine?.api.invertSelection(),
    });
    actions.register({
        id: 'growSelection',
        displayName: 'Grow Selection',
        category: 'selection',
        description: 'Expand the selection edge outward by a number of pixels.',
        icon: 'fa6-solid:up-right-and-down-left-from-center',
        menuPath: ['Select:50'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('grow'),
    });
    actions.register({
        id: 'shrinkSelection',
        displayName: 'Shrink Selection',
        category: 'selection',
        description: 'Contract the selection edge inward by a number of pixels.',
        icon: 'fa6-solid:down-left-and-up-right-to-center',
        menuPath: ['Select:60'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('shrink'),
    });
    actions.register({
        id: 'borderSelection',
        displayName: 'Border Selection',
        category: 'selection',
        description: 'Replace the selection with a band straddling its edge.',
        icon: 'fa6-solid:border-all',
        menuPath: ['Select:70'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('border'),
    });
    actions.register({
        id: 'smoothSelection',
        displayName: 'Smooth Selection',
        category: 'selection',
        description: 'Round off jagged edges and remove small specks.',
        icon: 'fa6-solid:wand-magic-sparkles',
        menuPath: ['Select:80'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => app.engine?.api.smoothSelection({ radius: 2 }),
    });
    actions.register({
        id: 'featherSelection',
        displayName: 'Feather Selection',
        category: 'selection',
        description: 'Soften the selection edge with a Gaussian blur.',
        icon: 'fa6-solid:feather',
        menuPath: ['Select:90'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => selectionModify.show('feather'),
    });
    actions.register({
        id: 'antialiasSelection',
        displayName: 'Antialias Selection',
        category: 'selection',
        description: 'Soften the staircase of a hard-edged selection.',
        icon: 'fa6-solid:wand-magic',
        menuPath: ['Select:100'],
        enabled: () => app.engineState?.hasSelection || 'No active selection',
        handler: () => app.engine?.api.antialiasSelection(),
    });

    // -- Image (canvas) --
    actions.register({
        id: 'resizeCanvas',
        displayName: 'Resize Canvas',
        category: 'edit',
        description: 'Resize the canvas with a 9-point anchor.',
        icon: 'fa6-solid:up-right-and-down-left-from-center',
        menuPath: ['Image:10'],
        handler: () => {
            if (!app.engine) return;
            resizeCanvas.open = true;
        },
    });
    actions.register({
        id: 'rescaleImage',
        displayName: 'Scale Image to New Size',
        category: 'edit',
        description: 'Resize all layers to new document dimensions.',
        icon: 'fa6-solid:expand',
        menuPath: ['Image:11'],
        handler: () => {
            if (!app.engine) return;
            imageRescale.open = true;
        },
    });
    actions.register({
        id: 'cropToSelection',
        displayName: 'Crop to Selection',
        category: 'edit',
        description: 'Crop the canvas to the current selection bounds.',
        icon: 'fa6-solid:crop-simple',
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
        displayName: 'Flip Canvas Horizontally',
        category: 'edit',
        description: 'Mirror the whole canvas left-to-right.',
        icon: 'fa6-solid:arrows-left-right',
        menuPath: ['Image:30'],
        handler: async () => {
            app.engine?.api.flipCanvas({ axis: 'h' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'flipCanvasV',
        displayName: 'Flip Canvas Vertically',
        category: 'edit',
        description: 'Mirror the whole canvas top-to-bottom.',
        icon: 'fa6-solid:arrows-up-down',
        menuPath: ['Image:31'],
        handler: async () => {
            app.engine?.api.flipCanvas({ axis: 'v' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'rotateCanvasCW',
        displayName: 'Rotate Canvas 90° CW',
        category: 'edit',
        description: 'Rotate the whole canvas a quarter turn clockwise.',
        icon: 'fa6-solid:rotate-right',
        menuPath: ['Image:40'],
        handler: async () => {
            app.engine?.api.rotateCanvas({ dir: 'cw' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'rotateCanvasCCW',
        displayName: 'Rotate Canvas 90° CCW',
        category: 'edit',
        description: 'Rotate the whole canvas a quarter turn counter-clockwise.',
        icon: 'fa6-solid:rotate-left',
        menuPath: ['Image:41'],
        handler: async () => {
            app.engine?.api.rotateCanvas({ dir: 'ccw' });
            await app.syncCanvasRect();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'rotateCanvas180',
        displayName: 'Rotate Canvas 180°',
        category: 'edit',
        description: 'Rotate the whole canvas a half turn.',
        icon: 'fa6-solid:rotate',
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
        displayName: 'Save',
        category: 'file',
        description:
            'Save the current document as a `.darkly` file. ' +
            'Re-saves to the same file after the first Save As; otherwise prompts.',
        icon: 'fa6-solid:floppy-disk',
        menuPath: ['File:30'],
        enabled: () => canSave || NO_SAVE_TOOLTIP,
        handler: () => {
            if (!app.engine) return;
            void saveDocument({ forceAs: false });
        },
    });
    actions.register({
        id: 'saveDocumentAs',
        displayName: 'Save As',
        category: 'file',
        description: 'Save the current document to a new `.darkly` file.',
        icon: 'fa6-solid:file-export',
        menuPath: ['File:40'],
        enabled: () => canSave || NO_SAVE_TOOLTIP,
        handler: () => {
            if (!app.engine) return;
            void saveDocument({ forceAs: true });
        },
    });
    actions.register({
        id: 'newDocument',
        displayName: 'New',
        category: 'file',
        description:
            'Open a fresh document in a new tab. Prompts for canvas size and background color.',
        icon: 'fa6-solid:file',
        menuPath: ['File:10'],
        // No default hotkey — `$mod+KeyN` is reserved by every major browser
        // for "new window" and cannot be intercepted by the page. Users can
        // still bind it via the Hotkeys tab if their browser/OS allows.
        handler: () => {
            newDocument.open = true;
        },
    });
    actions.register({
        id: 'open',
        displayName: 'Open',
        category: 'file',
        description:
            'Open a `.darkly` document or image (PNG / JPEG / WebP) in a new tab.',
        icon: 'fa6-solid:folder-open',
        menuPath: ['File:20'],
        handler: () => {
            void openFlow();
        },
    });
    actions.register({
        id: 'exportImage',
        displayName: 'Export Image…',
        category: 'file',
        description: 'Export the canvas composite as PNG, JPEG, or WebP.',
        icon: 'fa6-solid:image',
        menuPath: ['File:50'],
        handler: () => {
            if (!app.engine) return;
            exportImage.open = true;
        },
    });
    actions.register({
        id: 'exportTimelapse',
        displayName: 'Export Timelapse…',
        category: 'file',
        description: 'Export the process recording as an MP4 or GIF timelapse.',
        icon: 'fa6-solid:video',
        menuPath: ['File:51'],
        handler: () => {
            if (!app.engine) return;
            exportTimelapse.open = true;
        },
    });
    // -- Floating content / transform --
    actions.register({
        id: 'commitFloating',
        displayName: 'Commit Floating',
        category: 'transform',
        icon: 'fa6-solid:check',
        handler: () => {
            if (!app.engine) return;
            app.engine.api.commitFloating();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'cancelFloating',
        displayName: 'Cancel Floating',
        category: 'transform',
        icon: 'fa6-solid:xmark',
        handler: () => {
            if (!app.engine) return;
            app.engine.api.cancelFloating();
            app.requestFrame();
        },
    });

    // -- Tools (generated from registry) --
    // Tool key bindings come from the YAML preset layers (defaults.yaml +
    // overlay) via `hotkeys.<toolHotkeyAction>` — actions register without
    // any built-in default; the binding is purely configuration.
    // Tool display names live in Rust (`ToolRegistration`). Resolve through
    // `app.toolDisplayName(id)` which reads the registry map populated by
    // `app.loadRegistries(handle)` during editor init — the frontend never
    // hardcodes a label.
    for (const tool of toolRegistry.all()) {
        const name = app.toolDisplayName(tool.id);
        actions.register({
            id: tool.hotkeyAction,
            displayName: name,
            category: 'tools',
            description: `Switch to ${name} tool`,
            icon: glyphFromTool(tool),
            handler: () => { app.activeToolId = tool.id; },
        });
    }

    // Erase mode is a flag on the brush tool, not a tool of its own.
    // Hitting the hotkey from any other tool flips to brush AND turns
    // erase on (matches Krita's "E from anywhere paints with the eraser").
    actions.register({
        id: 'toggleEraseMode',
        displayName: 'Toggle Erase Mode',
        category: 'tools',
        description: 'Toggle erase mode on the brush tool. Switches to the brush tool first if another tool is active.',
        icon: 'fa6-solid:eraser',
        status: () => (brushSession.eraseMode ? 'fa6-solid:check' : undefined),
        handler: () => {
            if (app.activeToolId !== 'brush') {
                app.activeToolId = 'brush';
            }
            // No-op when the active brush's terminal opts out of erase
            // (smudge, liquify, watercolor). Same reason the BrushOptions
            // toggle is hidden — flipping `gpu.blend_mode` would do
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
        displayName: 'New Layer',
        category: 'layers',
        description: 'Add a new layer above the active one.',
        icon: 'fa6-solid:square-plus',
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
        id: 'newGroup',
        displayName: 'New Group',
        category: 'layers',
        description: 'Group the selected layers together, or add an empty group if nothing is selected.',
        icon: 'fa6-solid:folder-plus',
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
        displayName: 'Toggle Layer Visibility',
        category: 'layers',
        description: 'Show or hide the active layer.',
        icon: 'fa6-solid:eye',
        menuPath: ['Layer:70'],
        accepts: ['layerId'],
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
        displayName: 'Toggle Layer Lock',
        category: 'layers',
        description: 'Lock or unlock the active layer.',
        icon: 'fa6-solid:lock',
        menuPath: ['Layer:80'],
        accepts: ['layerId'],
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
        displayName: 'Isolate Layer',
        category: 'layers',
        description: 'Solo a layer so only it shows in the canvas. Press again to bring everything else back.',
        icon: 'fa6-solid:circle-dot',
        menuPath: ['Layer:90'],
        accepts: ['layerId'],
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.engine) return;
            toggleIsolation(layerId);
        },
    });

    actions.register({
        id: 'deleteLayer',
        displayName: 'Delete Layer',
        category: 'layers',
        description: 'Delete the selected layers.',
        icon: 'fa6-solid:trash',
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
            // `app.selectedLayerIds` here picks up exactly what the user
            // expects. We do NOT accept a `ctx.layerId` override — the
            // v1 attempt did, and that's what made "Delete N Layers" act
            // on just one layer.
            const targets = app.selectedLayerIds.size > 0
                ? [...app.selectedLayerIds]
                : app.activeLayerId !== null ? [app.activeLayerId] : [];
            if (targets.length === 0) return;
            try {
                // Stop any associated MediaStreams (camera / screenshare)
                // before the layers go away. `refreshLayerTree` reaps as a
                // safety net, but stopping eagerly turns off the OS capture
                // indicator immediately.
                for (const id of targets) app.stopMediaStreamVoid(id);
                if (targets.length === 1) {
                    await engine.api.removeLayer({ id: targets[0] });
                    app.clearSelection();
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
        displayName: 'Duplicate Layer',
        category: 'layers',
        description: 'Make a copy of each selected layer.',
        icon: 'fa6-solid:clone',
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
        displayName: 'Flip Horizontally',
        category: 'layers',
        description: 'Mirror the active layer (or selection) left-to-right.',
        icon: 'fa6-solid:arrows-left-right',
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
        displayName: 'Flip Vertically',
        category: 'layers',
        description: 'Mirror the active layer (or selection) top-to-bottom.',
        icon: 'fa6-solid:arrows-up-down',
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
    // from the Rust filter-pipeline registry (fetched into `app.filterTypes`
    // during `loadRegistries`), so a new filter in the core surfaces a
    // Colors-menu entry with no frontend edit. The target is the active *node*
    // (`activeLayerId` is the mask filter id when a mask is selected), which
    // is what makes "invert the mask" reachable from the same entry.
    for (const flt of app.filterTypes ?? []) {
        const filterType = flt.type;
        // A parametric filter (curves/levels/hsv) can't apply in one click — its
        // params must be authored first, so it opens the modal (the same
        // `FilterParamsEditor` the layer panel uses). Param-free filters (invert)
        // apply immediately.
        const parametric = (flt.params?.length ?? 0) > 0;
        actions.register({
            id: `filter${filterType.charAt(0).toUpperCase()}${filterType.slice(1)}`,
            displayName: parametric ? `${flt.displayName}…` : flt.displayName,
            category: 'layers',
            description: `Apply "${flt.displayName}" to the active layer or mask (respecting any selection).`,
            icon: 'fa6-solid:circle-half-stroke',
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
                        (flt.params ?? []) as unknown as FilterParam[]
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
        displayName: 'Merge Down',
        category: 'layers',
        description: 'Merge the active layer into the one below it, or combine multiple selected layers into a single layer.',
        icon: 'fa6-solid:arrows-down-to-line',
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
        displayName: 'Flatten',
        category: 'layers',
        description:
            'Bake modifiers into the layer (apply mask), or flatten a group into a single raster that inherits the group’s blend props.',
        icon: 'fa6-solid:layer-group',
        menuPath: ['Layer:120'],
        accepts: ['layerId'],
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
        displayName: 'Add Mask',
        category: 'layers',
        description: 'Add a mask modifier to the active layer or group and activate it for painting.',
        icon: 'radix-icons:mask-on',
        menuPath: ['Layer:100'],
        accepts: ['layerId'],
        handler: async (ctx) => {
            const engine = app.engine;
            if (!engine) return;
            const hostId = ctx.layerId ?? app.activeLayerId;
            if (hostId == null) return;
            engine.api.addMask({ id: hostId });
            // `add_mask` doesn't return the new modifier id, and we want
            // the mask to be the active paint target after creation —
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
        displayName: 'Settings',
        category: 'view',
        description: 'Show the preferences modal.',
        icon: 'fa6-solid:gear',
        // No `menuPath`: surfaced as the gear button on the menu bar and a
        // root courtesy item in the hamburger, not as a View submenu row.
        handler: () => { settings.open = true; },
    });

    actions.register({
        id: 'mirrorViewH',
        displayName: 'Mirror View',
        category: 'view',
        description: 'Flip the canvas horizontally for fresh-eyes review. View-only — the document is unchanged.',
        icon: 'fa6-solid:left-right',
        menuPath: ['View:10'],
        status: () => (app.mirrorH ? 'fa6-solid:check' : undefined),
        handler: () => {
            app.mirrorH = !app.mirrorH;
            app.requestFrame();
        },
    });

    actions.register({
        id: 'resetView',
        displayName: 'Reset View',
        category: 'view',
        description: 'Reset rotation, mirror, pan, and zoom-to-fit. View-only — the document is unchanged.',
        icon: 'fa6-solid:expand',
        menuPath: ['View:11'],
        handler: () => { app.resetView(); },
    });

    actions.register({
        id: 'fitToScreen',
        displayName: 'Fit to Screen',
        category: 'view',
        description: 'Zoom and recenter so the whole canvas fills the viewport, keeping the current rotation and mirror. View-only — the document is unchanged.',
        icon: 'fa6-solid:maximize',
        menuPath: ['View:12'],
        handler: () => { app.fitToScreen(); },
    });

    actions.register({
        id: 'centerView',
        displayName: 'Center View',
        category: 'view',
        description: 'Recenter the canvas in the viewport without changing zoom, rotation, or mirror. View-only — the document is unchanged.',
        icon: 'fa6-solid:crosshairs',
        menuPath: ['View:13'],
        handler: () => { app.centerView(); },
    });

    actions.register({
        id: 'commandPalette',
        displayName: 'Command Palette',
        category: 'view',
        description: 'Search and run any command.',
        icon: 'fa6-solid:magnifying-glass',
        // No `menuPath`: surfaced as the prominent "Find" item at the top of
        // the hamburger / on the menu bar, not as a buried submenu row.
        handler: () => { commandPalette.open = true; },
    });

    actions.register({
        id: 'openCheatsheet',
        displayName: 'Hotkey Cheat Sheet',
        category: 'view',
        description: 'Open a searchable, printable list of every keyboard shortcut.',
        icon: 'fa6-solid:keyboard',
        menuPath: ['Help:10'],
        handler: () => openCheatsheet(),
    });

    actions.register({
        id: 'openDocs',
        displayName: 'Documentation',
        category: 'view',
        description: 'Open the Darkly documentation in a new tab.',
        icon: 'fa6-solid:book',
        menuPath: ['Help:20'],
        handler: () => openExternal(links.docs),
    });

    actions.register({
        id: 'openWebsite',
        displayName: 'Website',
        category: 'view',
        description: 'Open the Darkly website in a new tab.',
        icon: 'fa6-solid:globe',
        menuPath: ['Help:30'],
        handler: () => openExternal(links.website),
    });

    actions.register({
        id: 'openGithub',
        displayName: 'GitHub Repository',
        category: 'view',
        description: 'Open the Darkly source repository on GitHub.',
        icon: 'fa6-brands:github',
        menuPath: ['Help:40'],
        handler: () => openExternal(links.github),
    });

    actions.register({
        id: 'aboutDarkly',
        displayName: 'About Darkly',
        category: 'view',
        description: 'Show version and credits.',
        icon: 'fa6-solid:circle-info',
        menuPath: ['Help:50'],
        handler: () => { about.open = true; },
    });

    // -- Brush parameters (size hotkeys + shift+drag scrub) --
    registerBrushParamActions();

    // -- Modifier-held color picker (Ctrl+drag → sample color) --
    registerSampleColorAction();

    // -- Clone brush set-source gesture (brush-scoped modifier+drag) --
    registerCloneSourceAction();

    // -- Brush builder --
    actions.register({
        id: 'addBrushNode',
        displayName: 'Add Brush Node',
        category: 'brush',
        description: 'Open the add-node menu at the cursor (brush builder).',
        icon: 'fa6-solid:diagram-project',
        handler: () => {
            // No-op if the brush builder isn't visible. The actual placement
            // — at the cursor in canvas coords — happens in NodeCanvas, which
            // owns pan/zoom and the cursor; we just signal it via an event.
            if (!brushGraph.isOpen) return;
            window.dispatchEvent(new CustomEvent('darkly:add-node-request'));
        },
    });
}

// -- Layer isolation --
//
// Isolation is pure session state — the engine's `isolated_node` is the
// single source of truth. We never touch `set_layer_visible` here, so eye
// icons stay independent: a user can toggle visibility on hidden siblings
// while soloed and those changes persist after un-solo.

function toggleIsolation(targetId: number) {
    const engine = app.engine;
    if (!engine) return;
    const next = app.isolatedNodeId === targetId ? null : targetId;
    engine.api.setIsolatedNode({ id: next });
    app.isolatedNodeId = next;
    app.requestFrame();
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
