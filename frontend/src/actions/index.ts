import { actions, sites } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { settings } from '../state/settings.svelte';
import { exportImage } from '../state/exportImage.svelte';
import { toolRegistry } from '../tools/registry';
import { copyToSystemClipboard, readImageFromClipboard, readLayerFromClipboard } from '../clipboard';
import { brushGraph } from '../state/brush_graph.svelte';
import { brushSession } from '../tools/brush.svelte';
import { registerBrushParamActions } from './brush_params';
import { screenToCanvas } from '../canvas/coordinates';

/** Hidden `<input type="file">` mounted by `App.svelte`. The `open-image`
 *  action triggers it; the change handler routes the file through
 *  `paste_image`. Wired by `App.svelte`'s `bind:this` so the action can
 *  fire .click() without owning the DOM node itself. */
let openImageInputEl: HTMLInputElement | null = null;

export function setOpenImageInput(el: HTMLInputElement | null) {
    openImageInputEl = el;
}

/** Decode an image file via the browser's native codecs and paste it as
 *  a new raster layer. Used by both the hidden file input and any future
 *  drag-and-drop hook. Returns the new layer id, or `-1` on decode failure. */
export async function openImageFile(file: File): Promise<number> {
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
        console.error('[open-image] decode failed', e);
        return -1;
    }
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
    sites.register({ name: 'keyboard',     provides: ['layerId'] });
    sites.register({ name: 'layerEye',     provides: ['layerId'] });
    sites.register({ name: 'layerThumb',   provides: ['layerId'] });
    sites.register({ name: 'maskThumb',    provides: ['layerId', 'maskIndex'] });
    sites.register({ name: 'canvas',       provides: ['x', 'y'] });

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

    // -- File I/O (image only — `.darkly` save/open lands in a later phase) --
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
    actions.register({
        id: 'openImage',
        displayName: 'Open Image…',
        category: 'file',
        description: 'Open a PNG / JPEG / WebP as a new raster layer.',
        defaultHotkey: '$mod+KeyO',
        handler: () => {
            if (!openImageInputEl) return;
            // Reset value so re-picking the same file still fires `change`.
            openImageInputEl.value = '';
            openImageInputEl.click();
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

    // -- View --
    actions.register({
        id: 'openSettings',
        displayName: 'Open Settings',
        category: 'view',
        description: 'Show the preferences modal.',
        defaultHotkey: '$mod+Comma',
        handler: () => { settings.open = true; },
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
