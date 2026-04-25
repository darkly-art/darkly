import { actions, sites } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { settings } from '../state/settings.svelte';
import { toolRegistry } from '../tools/registry';
import { copyToSystemClipboard, readImageFromClipboard } from '../clipboard';
import { brushGraph } from '../state/brush_graph.svelte';

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
                const layerId = app.handle.paste_image(
                    clip.width, clip.height, clip.rgba, ox, oy, activeId,
                );
                app.activeLayerId = layerId;
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
            const ok = app.handle.paste_in_place_floating(app.activeLayerId);
            console.log('[pasteInPlace] floating created:', ok, 'layerId:', app.activeLayerId);
            if (ok) {
                const prevTool = toolRegistry.get(app.activeToolId);
                app.activeToolId = 'transform';
                const ctx = { handle: app.handle, canvasEl: document.createElement('canvas'), screenToCanvas: (_x: number, _y: number) => ({ x: 0, y: 0 }) };
                prevTool?.onDeactivate?.(ctx);
                toolRegistry.get('transform')?.onActivate?.(ctx);
                app.requestFrame();
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
        description: 'Solo this layer, hiding all others. Press again to restore.',
        accepts: ['layerId'],
        defaultHotkey: 'KeyI',
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.handle) return;
            toggleIsolation(layerId, false);
        },
    });

    actions.register({
        id: 'isolateMask',
        displayName: 'Isolate Mask',
        category: 'layers',
        description: 'Solo this layer and show its mask as grayscale. Press again to restore.',
        accepts: ['layerId'],
        // No default keyboard or mouse trigger — Photoshop's preset turns on
        // the alt+click default on `maskThumb`. Krita-style users have no
        // mouse trigger by default.
        handler: (ctx) => {
            const layerId = ctx.layerId ?? app.activeLayerId;
            if (layerId == null || !app.handle) return;
            toggleIsolation(layerId, true);
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
}

// -- Layer isolation state --

interface IsolationSnapshot {
    visibility: Map<number, boolean>;
    /** If mask isolation was active, the layer whose show_mask was toggled on. */
    showMaskLayerId: number | null;
}

let isolationState: IsolationSnapshot | null = null;

/** @param showMask - force mask view (isolateMask action or keyboard isolate while editing mask) */
function toggleIsolation(targetId: number, showMask: boolean) {
    const handle = app.handle;
    if (!handle) return;

    if (isolationState !== null) {
        // Restore previous visibility
        for (const [id, wasVisible] of isolationState.visibility) {
            handle.set_layer_visible(id, wasVisible);
        }
        // Restore show_mask if we toggled it on
        if (isolationState.showMaskLayerId !== null) {
            handle.set_show_mask(isolationState.showMaskLayerId, false);
        }
        isolationState = null;
    } else {
        // Save current visibility, then solo the target.
        // The target + its ancestor groups must stay visible for it to render.
        const ancestorIds = findAncestorIds(app.layerTree, targetId) ?? [];
        const keepVisible = new Set([targetId, ...ancestorIds]);
        const allLayers = collectLayers(app.layerTree);
        const visibility = new Map<number, boolean>();
        for (const layer of allLayers) {
            visibility.set(layer.id, layer.visible);
            handle.set_layer_visible(layer.id, keepVisible.has(layer.id));
        }

        // Show mask as grayscale if requested (explicit isolateMask action,
        // or keyboard isolateLayer while editing a mask)
        let showMaskLayerId: number | null = null;
        const wantMask = showMask || app.editingMaskLayerId === targetId;
        if (wantMask) {
            const layer = findLayer(app.layerTree, targetId);
            if (layer?.hasMask && !layer.showMask) {
                handle.set_show_mask(targetId, true);
                showMaskLayerId = targetId;
            }
        }

        isolationState = { visibility, showMaskLayerId };
    }
    app.refreshLayerTree();
    app.requestFrame();
}

/** Find the chain of ancestor group IDs for a given layer.
 *  Returns null if the target is not found in this subtree. */
function findAncestorIds(tree: any[], targetId: number): number[] | null {
    for (const node of tree) {
        if (node.id === targetId) return [];
        if (node.children) {
            const found = findAncestorIds(node.children, targetId);
            if (found !== null) return [node.id, ...found];
        }
    }
    return null;
}

/** Flatten the layer tree into a list of { id, visible } records. */
function collectLayers(tree: any[]): { id: number; visible: boolean }[] {
    const result: { id: number; visible: boolean }[] = [];
    for (const node of tree) {
        result.push({ id: node.id, visible: node.visible });
        if (node.children) {
            result.push(...collectLayers(node.children));
        }
    }
    return result;
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
