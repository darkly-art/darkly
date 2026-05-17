import { actions, sites } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { settings } from '../state/settings.svelte';
import { exportImage } from '../state/exportImage.svelte';
import { loadError, parseLoadErrorMessage } from '../state/loadError.svelte';
import { toast } from '../state/toast.svelte';
import { toolRegistry } from '../tools/registry';
import { copyToSystemClipboard, readImageFromClipboard, readLayerFromClipboard } from '../clipboard';
import { brushGraph } from '../state/brush_graph.svelte';
import { brushSession } from '../tools/brush.svelte';
import { registerBrushParamActions } from './brush_params';
import { screenToCanvas } from '../canvas/coordinates';
import { pickOpenFile, type OpenedFile } from '../storage/fileHandle';
import { detectKind, isImageKind, type FileKind } from '../storage/detectKind';
import { saveDocument } from '../storage/saveDocument';
import { shell } from '../multi_tab/shell.svelte';

/** Strip the file extension from a picker-supplied name so we can use
 *  it as a tab title. Matches the basename-only convention already used
 *  by Save As (which seeds `set_document_name` from the chosen filename
 *  minus `.darkly`). */
function tabNameFromFile(fileName: string): string {
    const stripped = fileName.replace(/\.[^./]+$/, '');
    return stripped || 'Untitled';
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
    inst.onHandleReady = (handle) => {
        try {
            handle.open_document(picked.bytes);
            // Tab strip reads through `handle.document_name()` (which
            // the loader populated from `manifest.name`), but the
            // shell's `nameVersion` doesn't bump on its own — nudge
            // it so the strip re-derives.
            shell.setName(inst.id, handle.document_name());
            app.refreshLayerTree();
            app.refreshVeilList();
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
        bitmap = await createImageBitmap(new Blob([picked.bytes]));
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
    inst.onHandleReady = (handle) => {
        // Pass anchor = -1 (no specific layer) — the new tab has no
        // bg seed (the `onHandleReady` presence suppresses it), so
        // paste lands at the bottom of root, which is the only sensible
        // position for the doc's first layer.
        handle.paste_image(width, height, rgba, 0, 0, -1);
        app.refreshLayerTree();
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
    if (!app.handle) return -1;
    try {
        const bitmap = await createImageBitmap(file);
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const ctx = canvas.getContext('2d')!;
        ctx.drawImage(bitmap, 0, 0);
        bitmap.close();
        const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);
        const rgba = new Uint8Array(imageData.data.buffer);
        const activeId = app.activeLayerId ?? -1;
        const layerId = app.handle.paste_image(
            canvas.width,
            canvas.height,
            rgba,
            0,
            0,
            activeId,
        );
        app.selectLayer(layerId);
        app.refreshLayerTree();
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

function enterTransformTool() {
    if (!app.handle || !app.canvasEl) return;
    const wasTransform = app.activeToolId === 'transform';
    app.activeToolId = 'transform';
    // Tool changes are handled by the $effect in CanvasView, which calls
    // onDeactivate/onActivate. When the tool was already transform that
    // effect skips, so we must manually re-activate to sync state with
    // the new floating — but never call onDeactivate, since that would
    // commit the floating we just set up.
    if (wasTransform) {
        const canvasEl = app.canvasEl;
        const ctx = {
            handle: app.handle,
            canvasEl,
            screenToCanvas: (sx: number, sy: number) => screenToCanvas(sx, sy, canvasEl),
        };
        toolRegistry.get('transform')?.onActivate?.(ctx);
    }
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
        defaultHotkey: '$mod+KeyZ',
        handler: () => { app.handle?.undo(); app.refreshLayerTree(); },
    });
    actions.register({
        id: 'redo',
        displayName: 'Redo',
        category: 'edit',
        defaultHotkey: '$mod+Shift+KeyZ',
        handler: () => { app.handle?.redo(); app.refreshLayerTree(); },
    });

    // -- Colors --
    actions.register({
        id: 'resetColors',
        displayName: 'Reset Colors',
        category: 'colors',
        defaultHotkey: 'KeyD',
        handler: () => app.resetColors(),
    });
    actions.register({
        id: 'swapColors',
        displayName: 'Swap Colors',
        category: 'colors',
        defaultHotkey: 'KeyX',
        handler: () => app.swapColors(),
    });

    // -- Selection --
    actions.register({
        id: 'selectAll',
        displayName: 'Select All',
        category: 'selection',
        defaultHotkey: '$mod+KeyA',
        handler: () => app.handle?.select_all(),
    });
    actions.register({
        id: 'clearSelection',
        displayName: 'Clear Selection',
        category: 'selection',
        defaultHotkey: '$mod+Shift+KeyA',
        handler: () => app.handle?.clear_selection(),
    });
    actions.register({
        id: 'clearSelectionContents',
        displayName: 'Clear Selection Contents',
        category: 'selection',
        defaultHotkey: 'Delete',
        handler: () => {
            if (app.activeLayerId != null) {
                app.handle?.clear_selection_contents(app.activeLayerId);
            }
        },
    });
    actions.register({
        id: 'invertSelection',
        displayName: 'Invert Selection',
        category: 'selection',
        defaultHotkey: '$mod+Shift+KeyI',
        handler: () => app.handle?.invert_selection(),
    });

    // -- Clipboard --
    actions.register({
        id: 'copy',
        displayName: 'Copy',
        category: 'edit',
        defaultHotkey: '$mod+KeyC',
        handler: () => {
            if (!app.handle || app.activeLayerId == null) return;
            const handle = app.handle;
            // `copy_layer_rich` snapshots metadata up front and then drives
            // the same async pixel readback that `copy` does — it's a
            // superset, so we don't need to call both.
            handle.copy_layer_rich(app.activeLayerId);
            app.onCopyResult((result) => {
                if (!result?.rgba) return;
                // The rich JSON lands one frame later, on the same readback
                // completion path. Polling here is safe because we got the
                // pixel result; the rich result is set before this callback.
                const richJson = handle.poll_copy_rich_result() ?? undefined;
                copyToSystemClipboard(result.rgba, result.width, result.height, richJson);
            });
        },
    });
    actions.register({
        id: 'cut',
        displayName: 'Cut',
        category: 'edit',
        defaultHotkey: '$mod+KeyX',
        handler: () => {
            if (!app.handle || app.activeLayerId == null) return;
            const handle = app.handle;
            // No `cut_layer_rich` yet — fall back to the pixels-only path
            // for cut. Cross-tab paste of a cut layer still works (PNG
            // fallback restores the bitmap) but loses blend mode/opacity.
            // Worth a follow-up.
            handle.cut(app.activeLayerId);
            app.onCopyResult((result) => {
                if (result?.rgba) {
                    copyToSystemClipboard(result.rgba, result.width, result.height);
                }
            });
            app.requestFrame();
        },
    });
    actions.register({
        id: 'paste',
        displayName: 'Paste',
        category: 'edit',
        defaultHotkey: '$mod+KeyV',
        handler: async () => {
            if (!app.handle) return;

            // Prefer the rich-layer payload if a Darkly tab put one on the
            // clipboard. Cross-tab paste this way preserves blend mode and
            // opacity, which the PNG fallback cannot. Brush-builder pastes
            // always want the pixel path, so skip rich there.
            if (!brushGraph.isOpen) {
                const rich = await readLayerFromClipboard();
                if (rich && app.handle) {
                    const activeId = app.activeLayerId ?? -1;
                    const layerId = app.handle.paste_layer_rich(rich, activeId);
                    if (layerId >= 0) {
                        app.selectLayer(layerId);
                        const activateTransform =
                            config.get('edit.activateTransformAfterPaste') !== false;
                        if (activateTransform) enterTransformTool();
                        app.refreshLayerTree();
                        app.requestFrame();
                        return;
                    }
                    // Rich paste failed (malformed JSON, bad pixel data) —
                    // fall through to the PNG path below.
                }
            }

            readImageFromClipboard().then(clip => {
                if (!clip || !app.handle) return;

                // If the brush builder is open, paste into the node editor
                // instead of the main canvas.  Fill the selected Image node
                // when there is one; otherwise spawn a new Image node.
                if (brushGraph.isOpen) {
                    let nodeId: number | null = null;
                    if (brushGraph.selectedNode != null) {
                        const node = brushGraph.graph?.nodes[String(brushGraph.selectedNode)];
                        if (node?.type_id === 'image') nodeId = brushGraph.selectedNode;
                    }
                    if (nodeId == null) {
                        const count = brushGraph.nodeList.length;
                        const x = 100 + (count % 4) * 180;
                        const y = 50 + Math.floor(count / 4) * 120;
                        nodeId = brushGraph.addNode('image', x, y);
                    }
                    if (nodeId != null) {
                        brushGraph.uploadImageToNode(
                            nodeId,
                            `image_${nodeId}`,
                            clip.rgba,
                            clip.width,
                            clip.height,
                        );
                        brushGraph.selectedNode = nodeId;
                        return;
                    }
                }

                const docW = config.get('canvas.width') as number;
                const docH = config.get('canvas.height') as number;
                const ox = Math.round((docW - clip.width) / 2);
                const oy = Math.round((docH - clip.height) / 2);
                const activeId = app.activeLayerId ?? -1;
                const activateTransform = config.get('edit.activateTransformAfterPaste') !== false;
                if (activateTransform) {
                    const layerId = app.handle.paste_image_floating(
                        clip.width, clip.height, clip.rgba, ox, oy, activeId,
                    );
                    app.selectLayer(layerId);
                    enterTransformTool();
                } else {
                    const layerId = app.handle.paste_image(
                        clip.width, clip.height, clip.rgba, ox, oy, activeId,
                    );
                    app.selectLayer(layerId);
                }
                app.refreshLayerTree();
                app.requestFrame();
            });
        },
    });
    actions.register({
        id: 'pasteInPlace',
        displayName: 'Paste in Place',
        category: 'edit',
        defaultHotkey: '$mod+Shift+KeyV',
        handler: () => {
            if (!app.handle || app.activeLayerId == null) return;
            const activateTransform = config.get('edit.activateTransformAfterPaste') !== false;
            if (activateTransform) {
                const ok = app.handle.paste_in_place_floating(app.activeLayerId);
                if (ok) {
                    enterTransformTool();
                    app.requestFrame();
                }
            } else {
                const layerId = app.handle.paste_in_place(app.activeLayerId);
                if (layerId >= 0) {
                    app.selectLayer(layerId);
                    app.refreshLayerTree();
                    app.requestFrame();
                }
            }
        },
    });

    // -- File I/O --
    actions.register({
        id: 'saveDocument',
        displayName: 'Save',
        category: 'file',
        description:
            'Save the current document as a `.darkly` file. ' +
            'Re-saves to the same file after the first Save As; otherwise prompts.',
        defaultHotkey: '$mod+KeyS',
        handler: () => {
            if (!app.handle) return;
            void saveDocument({ forceAs: false });
        },
    });
    actions.register({
        id: 'saveDocumentAs',
        displayName: 'Save As',
        category: 'file',
        description: 'Save the current document to a new `.darkly` file.',
        defaultHotkey: '$mod+Shift+KeyS',
        handler: () => {
            if (!app.handle) return;
            void saveDocument({ forceAs: true });
        },
    });
    actions.register({
        id: 'open',
        displayName: 'Open',
        category: 'file',
        description:
            'Open a `.darkly` document or image (PNG / JPEG / WebP) in a new tab.',
        defaultHotkey: '$mod+KeyO',
        handler: () => {
            void openFlow();
        },
    });
    actions.register({
        id: 'exportImage',
        displayName: 'Export Image…',
        category: 'file',
        description: 'Export the canvas composite as PNG, JPEG, or WebP.',
        defaultHotkey: '$mod+Shift+KeyE',
        handler: () => {
            if (!app.handle) return;
            exportImage.open = true;
        },
    });
    // -- Floating content / transform --
    actions.register({
        id: 'commitFloating',
        displayName: 'Commit Floating',
        category: 'transform',
        defaultHotkey: 'Enter',
        handler: () => {
            if (!app.handle) return;
            app.handle.commit_floating();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'cancelFloating',
        displayName: 'Cancel Floating',
        category: 'transform',
        defaultHotkey: 'Escape',
        handler: () => {
            if (!app.handle) return;
            app.handle.cancel_floating();
            app.requestFrame();
        },
    });

    // -- Tools (generated from registry) --
    // Tools' default hotkeys live alongside the action registration here
    // (rather than on the Tool interface) so all action defaults stay
    // co-located. Override these via `hotkeys.<toolHotkeyAction>` in config.
    const TOOL_DEFAULT_HOTKEYS: Record<string, string> = {
        brushTool: 'KeyB',
        fillTool: 'KeyF',
        gradientTool: 'KeyG',
        colorPickerTool: 'KeyP',
        rectSelectTool: 'KeyR',
        ellipseSelectTool: 'Shift+KeyR',
        lassoSelectTool: 'KeyL',
        magicWandTool: 'KeyW',
        transformTool: 'KeyT',
    };
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
            defaultHotkey: TOOL_DEFAULT_HOTKEYS[tool.hotkeyAction],
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
        defaultHotkey: 'KeyE',
        handler: () => {
            if (app.activeToolId !== 'brush') {
                app.activeToolId = 'brush';
            }
            brushSession.eraseMode = !brushSession.eraseMode;
            app.handle?.set_brush_blend_mode(brushSession.eraseMode ? 1 : 0);
        },
    });

    // -- Layers --
    actions.register({
        id: 'toggleVisibility',
        displayName: 'Toggle Layer Visibility',
        category: 'layers',
        accepts: ['layerId'],
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.handle) return;
            const layer = findLayer(app.layerTree, layerId);
            if (layer) {
                app.handle.set_layer_visible(layerId, !layer.visible);
            }
        },
    });

    actions.register({
        id: 'isolateLayer',
        displayName: 'Isolate Layer',
        category: 'layers',
        description: 'Solo a layer so only it shows in the canvas. Press again to bring everything else back.',
        accepts: ['layerId'],
        defaultHotkey: 'KeyI',
        // Two defaults: one per thumbnail. The dispatching click handler
        // is responsible for putting the right node id (host vs mask
        // modifier) in the context — that's what makes "alt+click on a
        // preview = isolate that preview" work for both.
        defaultMouseClick: ['layerThumb:alt+click', 'maskThumb:alt+click'],
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.handle) return;
            toggleIsolation(layerId);
        },
    });

    actions.register({
        id: 'deleteLayer',
        displayName: 'Delete Layer',
        category: 'layers',
        description: 'Delete the active layer (or remove the active veil).',
        // Krita-style global default. Photoshop / GIMP presets override
        // this to `layerPanel:Delete` (panel-scoped) — see presets/.
        defaultHotkey: 'Shift+Delete',
        accepts: ['layerId'],
        handler: (ctx) => {
            if (!app.handle) return;
            // Veil takes priority: the trash button on the layer panel
            // doubles as veil-remove when a veil is active, so the
            // keyboard shortcut should too.
            if (app.activeVeilIndex !== null) {
                app.removeVeil(app.activeVeilIndex);
                return;
            }
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null) return;
            try {
                app.handle.remove_layer(layerId);
                app.clearSelection();
                app.refreshLayerTree();
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        },
    });

    actions.register({
        id: 'duplicateLayer',
        displayName: 'Duplicate Layer',
        category: 'layers',
        description: 'Make a copy of the active layer or group directly above it.',
        defaultHotkey: '$mod+KeyJ',
        accepts: ['layerId'],
        handler: (ctx) => {
            if (!app.handle) return;
            const sourceId = ctx.layerId ?? app.activeLayerId;
            if (sourceId == null) return;
            const newId = app.handle.duplicate_node(sourceId);
            app.refreshLayerTree();
            if (newId) app.selectLayer(newId);
        },
    });

    actions.register({
        id: 'mergeDown',
        displayName: 'Merge Down',
        category: 'layers',
        description: 'Merge the active layer or group into the layer below it.',
        defaultHotkey: '$mod+KeyE',
        accepts: ['layerId'],
        handler: (ctx) => {
            if (!app.handle) return;
            const sourceId = ctx.layerId ?? app.activeLayerId;
            if (sourceId == null) return;
            try {
                const newId = app.handle.merge_down(sourceId);
                app.refreshLayerTree();
                if (newId) app.selectLayer(newId);
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        },
    });

    actions.register({
        id: 'flattenImage',
        displayName: 'Flatten Image',
        category: 'layers',
        description: 'Composite every visible layer into a single "Background" raster; discard the rest.',
        defaultHotkey: '$mod+Shift+KeyE',
        handler: () => {
            if (!app.handle) return;
            try {
                const newId = app.handle.flatten_image();
                app.refreshLayerTree();
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
        accepts: ['layerId'],
        handler: (ctx) => {
            if (!app.handle) return;
            const id = ctx.layerId ?? app.activeLayerId;
            if (id == null) return;
            try {
                const newId = app.handle.flatten_node(id);
                app.refreshLayerTree();
                if (newId) app.selectLayer(newId);
            } catch (e: any) {
                toast.show('error', e.message ?? String(e));
            }
        },
    });

    // -- View --
    actions.register({
        id: 'openSettings',
        displayName: 'Open Settings',
        category: 'view',
        description: 'Show the preferences modal.',
        defaultHotkey: '$mod+Comma',
        handler: () => { settings.open = true; },
    });

    actions.register({
        id: 'mirrorViewH',
        displayName: 'Mirror View',
        category: 'view',
        description: 'Flip the canvas horizontally for fresh-eyes review. View-only — the document is unchanged.',
        defaultHotkey: 'KeyM',
        handler: () => {
            app.mirrorH = !app.mirrorH;
            app.requestFrame();
        },
    });

    // -- Brush parameters (size hotkeys + shift+drag scrub) --
    registerBrushParamActions();

    // -- Brush builder --
    actions.register({
        id: 'addBrushNode',
        displayName: 'Add Brush Node',
        category: 'brush',
        description: 'Open the add-node menu at the cursor (brush builder).',
        defaultHotkey: 'Shift+KeyA',
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
    const handle = app.handle;
    if (!handle) return;
    const next = app.isolatedNodeId === targetId ? 0 : targetId;
    handle.set_isolated_node(next);
    app.isolatedNodeId = next === 0 ? null : next;
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
