import { actions, sites } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { settings } from '../state/settings.svelte';
import { toolRegistry } from '../tools/registry';
import { copyToSystemClipboard, readImageFromClipboard } from '../clipboard';
import { brushGraph } from '../state/brush_graph.svelte';
import { registerBrushParamActions } from './brush_params';
import { screenToCanvas } from '../canvas/coordinates';

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
            app.handle.copy(app.activeLayerId);
            app.onCopyResult((result) => {
                if (result?.rgba) {
                    copyToSystemClipboard(result.rgba, result.width, result.height);
                }
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
            app.handle.cut(app.activeLayerId);
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
        handler: () => {
            if (!app.handle) return;
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
        eraserTool: 'KeyE',
        fillTool: 'KeyF',
        gradientTool: 'KeyG',
        colorPickerTool: 'KeyP',
        rectSelectTool: 'KeyR',
        ellipseSelectTool: 'Shift+KeyR',
        lassoSelectTool: 'KeyL',
        magicWandTool: 'KeyW',
        transformTool: 'KeyT',
    };
    for (const tool of toolRegistry.all()) {
        actions.register({
            id: tool.hotkeyAction,
            displayName: tool.name,
            category: 'tools',
            description: `Switch to ${tool.name} tool`,
            defaultHotkey: TOOL_DEFAULT_HOTKEYS[tool.hotkeyAction],
            handler: () => { app.activeToolId = tool.id; },
        });
    }

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
        description: 'Solo a layer (raster, group, or mask) by skipping off-path siblings in the compositor. Press again to restore. Pure session state — eye-icon visibility is preserved across toggles. Default chord fires from either thumbnail: alt+click on the layer thumb solos that layer; alt+click on the mask thumb solos the mask (renders as grayscale).',
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
