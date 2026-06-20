import { actions } from './registry';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { brushGraph } from '../state/brush_graph.svelte';
import { copyToSystemClipboard, readImageFromClipboard, readLayerFromClipboard } from '../clipboard';
import { toolRegistry, type ToolContext } from '../tools/registry';
import { screenToCanvas } from '../canvas/coordinates';

/** Switch to the transform tool after a paste so the freshly-floated layer is
 *  immediately draggable. When transform is already active we re-activate
 *  manually (the CanvasView `$effect` skips a no-op tool change) but never
 *  deactivate — that would commit the floating we just set up. */
function enterTransformTool() {
    if (!app.engine || !app.canvasEl) return;
    const wasTransform = app.activeToolId === 'transform';
    app.activeToolId = 'transform';
    if (wasTransform && app.engine && app.canvasEl) {
        const canvasEl = app.canvasEl;
        const ctx: ToolContext = {
            engine: app.engine,
            canvasEl,
            screenToCanvas: (sx: number, sy: number) => screenToCanvas(sx, sy, canvasEl),
        };
        toolRegistry.get('transform')?.onActivate?.(ctx);
    }
}

/** Copy / cut / paste of the active layer. Copy and cut readback the layer's
 *  pixels asynchronously and hand them to the system clipboard; paste prefers a
 *  rich Darkly-layer payload (blend mode + opacity preserved) and falls back to
 *  any image MIME on the clipboard. */
export function registerClipboardActions(): void {
    actions.register({
        id: 'copy',
        displayName: 'Copy',
        category: 'edit',
        description: 'Copy the active layer to the clipboard.',
        icon: 'fa6-solid:copy',
        menuPath: ['Edit:40'],
        handler: () => {
            const engine = app.engine;
            if (!engine || app.activeLayerId == null) return;
            // `copy_layer_rich` snapshots metadata up front and then drives
            // the same async pixel readback that `copy` does — it's a
            // superset, so we don't need to call both.
            engine.post('copy_layer_rich', { id: app.activeLayerId });
            app.onCopyResult(async (result) => {
                if (!result?.rgba) return;
                // The rich JSON lands one frame later, on the same readback
                // completion path. Polling here is safe because we got the
                // pixel result; the rich result is set before this callback.
                const richJson = (await engine.send('poll_copy_rich_result')) ?? undefined;
                copyToSystemClipboard(result.rgba, result.width, result.height, richJson);
            });
        },
    });
    actions.register({
        id: 'cut',
        displayName: 'Cut',
        category: 'edit',
        description: 'Cut the active layer to the clipboard.',
        icon: 'fa6-solid:scissors',
        menuPath: ['Edit:30'],
        handler: async () => {
            const engine = app.engine;
            if (!engine || app.activeLayerId == null) return;
            // No `cut_layer_rich` yet — fall back to the pixels-only path
            // for cut. Cross-tab paste of a cut layer still works (PNG
            // fallback restores the bitmap) but loses blend mode/opacity.
            // Worth a follow-up.
            await engine.send('cut', { id: app.activeLayerId });
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
        description: 'Paste an image or layer from the clipboard.',
        icon: 'fa6-solid:paste',
        menuPath: ['Edit:50'],
        handler: async () => {
            const engine = app.engine;
            if (!engine) return;

            // Prefer the rich-layer payload if a Darkly tab put one on the
            // clipboard. Cross-tab paste this way preserves blend mode and
            // opacity, which the PNG fallback cannot. Brush-builder pastes
            // always want the pixel path, so skip rich there.
            if (!brushGraph.isOpen) {
                const rich = await readLayerFromClipboard();
                if (rich) {
                    const activeId = app.activeLayerId ?? -1;
                    const { id: layerId } = await engine.send('paste_layer_rich', { json: rich, active_layer_id: activeId });
                    if (layerId >= 0) {
                        app.selectLayer(layerId);
                        const activateTransform =
                            config.get('edit.activateTransformAfterPaste') !== false;
                        if (activateTransform) enterTransformTool();
                        await app.refreshLayerTree();
                        app.requestFrame();
                        return;
                    }
                    // Rich paste failed (malformed JSON, bad pixel data) —
                    // fall through to the PNG path below.
                }
            }

            const clip = await readImageFromClipboard();
            if (!clip) return;

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
                    nodeId = await brushGraph.addNode('image', x, y);
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

            const ox = Math.round((app.docW - clip.width) / 2);
            const oy = Math.round((app.docH - clip.height) / 2);
            const activeId = app.activeLayerId ?? -1;
            const activateTransform = config.get('edit.activateTransformAfterPaste') !== false;
            if (activateTransform) {
                const { id: layerId } = await engine.send(
                    'paste_image_floating',
                    { width: clip.width, height: clip.height, offset_x: ox, offset_y: oy, active_layer_id: activeId },
                    clip.rgba,
                );
                app.selectLayer(layerId);
                enterTransformTool();
            } else {
                const { id: layerId } = await engine.send(
                    'paste_image',
                    { width: clip.width, height: clip.height, offset_x: ox, offset_y: oy, active_layer_id: activeId },
                    clip.rgba,
                );
                app.selectLayer(layerId);
            }
            await app.refreshLayerTree();
            app.requestFrame();
        },
    });
    actions.register({
        id: 'pasteInPlace',
        displayName: 'Paste in Place',
        category: 'edit',
        description: 'Paste from the clipboard at its original position.',
        icon: 'fa6-solid:clipboard',
        menuPath: ['Edit:60'],
        handler: async () => {
            const engine = app.engine;
            if (!engine || app.activeLayerId == null) return;
            const activateTransform = config.get('edit.activateTransformAfterPaste') !== false;
            if (activateTransform) {
                const { ok } = await engine.send('paste_in_place_floating', { id: app.activeLayerId });
                if (ok) {
                    enterTransformTool();
                    app.requestFrame();
                }
            } else {
                const { id: layerId } = await engine.send('paste_in_place', { active_layer_id: app.activeLayerId });
                if (layerId >= 0) {
                    app.selectLayer(layerId);
                    await app.refreshLayerTree();
                    app.requestFrame();
                }
            }
        },
    });
}
