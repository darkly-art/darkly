<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import BrushPreview from './BrushPreview.svelte';
    import NodeCanvas from './NodeCanvas.svelte';
    import NodePalette from './NodePalette.svelte';

    function handleAddNode(typeId: string) {
        // Place new nodes near the center of the canvas.
        // A simple offset based on how many nodes exist.
        const count = brushGraph.nodeList.length;
        const x = 100 + (count % 4) * 180;
        const y = 50 + Math.floor(count / 4) * 120;
        brushGraph.addNode(typeId, x, y);
    }

    function handleReset() {
        brushGraph.resetToDefault();
    }

    /** Measure all node widgets in the DOM and run auto-layout with real sizes. */
    function handleAutoLayout() {
        const sizes: Record<string, [number, number]> = {};
        for (const el of document.querySelectorAll<HTMLElement>('[data-node-id]')) {
            const id = el.dataset.nodeId;
            if (id) sizes[id] = [el.offsetWidth, el.offsetHeight];
        }
        brushGraph.autoLayout(sizes);
    }

    let fullscreen = $state(false);

    /** Brush preview visibility. Persisted per-browser via localStorage so the
     *  user's choice survives across sessions. */
    const PREVIEW_STORAGE_KEY = 'darkly.brush-preview-visible';
    function loadPreviewVisible(): boolean {
        try {
            const raw = localStorage.getItem(PREVIEW_STORAGE_KEY);
            if (raw === null) return true; // default on
            return raw !== '0';
        } catch {
            return true;
        }
    }
    let previewVisible = $state(loadPreviewVisible());
    function togglePreview() {
        previewVisible = !previewVisible;
        try {
            localStorage.setItem(PREVIEW_STORAGE_KEY, previewVisible ? '1' : '0');
        } catch {
            // Private mode / disabled storage — visibility still toggles
            // for the current session, just doesn't persist.
        }
    }

    // --- Resizable preview dimensions ---

    const PREVIEW_SIZE_STORAGE_KEY = 'darkly.brush-preview-size';
    const DEFAULT_PREVIEW_SIZE = { w: 320, h: 120 };
    /** Minimum renderable size — below this, the S-curve is too cramped to
     *  read. Upper bound keeps one careless drag from consuming the whole
     *  editor; WASM renders are still cheap at that size. */
    const MIN_W = 160, MIN_H = 60;
    const MAX_W = 800, MAX_H = 400;

    function loadPreviewSize(): { w: number; h: number } {
        try {
            const raw = localStorage.getItem(PREVIEW_SIZE_STORAGE_KEY);
            if (!raw) return { ...DEFAULT_PREVIEW_SIZE };
            const parsed = JSON.parse(raw);
            const w = Math.max(MIN_W, Math.min(MAX_W, Math.round(parsed?.w ?? DEFAULT_PREVIEW_SIZE.w)));
            const h = Math.max(MIN_H, Math.min(MAX_H, Math.round(parsed?.h ?? DEFAULT_PREVIEW_SIZE.h)));
            return { w, h };
        } catch {
            return { ...DEFAULT_PREVIEW_SIZE };
        }
    }

    let previewSize = $state(loadPreviewSize());

    let resizing = false;
    let startClientX = 0;
    let startClientY = 0;
    let startW = 0;
    let startH = 0;

    function startResize(e: PointerEvent) {
        // Left-button only — ignore right-click and middle-click.
        if (e.button !== 0) return;
        resizing = true;
        startClientX = e.clientX;
        startClientY = e.clientY;
        startW = previewSize.w;
        startH = previewSize.h;
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        // Suppress the engine's continuous render loop so resize pointer
        // events stay on the hot path.
        app.beginInteraction();
        e.preventDefault();
    }

    function onResizeMove(e: PointerEvent) {
        if (!resizing) return;
        // Dock is anchored bottom-right. Dragging up+left grows the box;
        // down+right shrinks it.
        const dx = e.clientX - startClientX;
        const dy = e.clientY - startClientY;
        const w = Math.max(MIN_W, Math.min(MAX_W, Math.round(startW - dx)));
        const h = Math.max(MIN_H, Math.min(MAX_H, Math.round(startH - dy)));
        if (w !== previewSize.w || h !== previewSize.h) {
            previewSize = { w, h };
        }
    }

    function endResize(e: PointerEvent) {
        if (!resizing) return;
        resizing = false;
        (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
        app.endInteraction();
        try {
            localStorage.setItem(
                PREVIEW_SIZE_STORAGE_KEY,
                JSON.stringify(previewSize),
            );
        } catch {
            // Private mode / disabled storage — size still applies for
            // this session, just doesn't persist.
        }
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Escape' && fullscreen) {
            fullscreen = false;
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="brush-builder" class:fullscreen>
    <div class="builder-toolbar">
        <span class="builder-title">Brush Builder</span>
        <NodePalette onaddnode={handleAddNode} />
        <button class="toolbar-btn" onclick={handleReset} title="Reset to default">Reset</button>
        <button class="toolbar-btn" onclick={handleAutoLayout} title="Auto-layout nodes">Layout</button>
        <div class="spacer"></div>
    </div>

    <div class="canvas-wrapper">
        <NodeCanvas />
        <div class="preview-dock">
            {#if previewVisible}
                <BrushPreview width={previewSize.w} height={previewSize.h} />
                <div
                    class="resize-handle"
                    onpointerdown={startResize}
                    onpointermove={onResizeMove}
                    onpointerup={endResize}
                    onpointercancel={endResize}
                    role="slider"
                    aria-label="Resize brush preview"
                    aria-valuenow={previewSize.w}
                    aria-valuemin={MIN_W}
                    aria-valuemax={MAX_W}
                    tabindex="-1"
                ></div>
                <button
                    class="close-btn"
                    onclick={togglePreview}
                    aria-label="Hide brush preview"
                    title="Hide brush preview"
                >
                    <i class="fa-solid fa-eye-slash"></i>
                </button>
            {:else}
                <button
                    class="bookmark"
                    onclick={togglePreview}
                    aria-label="Show brush preview"
                    title="Show brush preview"
                >Preview</button>
            {/if}
        </div>
        <button
            class="fullscreen-btn"
            onclick={() => fullscreen = !fullscreen}
            title={fullscreen ? "Exit fullscreen" : "Fullscreen"}
        >
            <i class={fullscreen ? 'fa-solid fa-compress' : 'fa-solid fa-expand'}></i>
        </button>
    </div>
</div>

<style>
    .brush-builder {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--bg);
    }
    .builder-toolbar {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px;
        background: var(--bg);
        border-bottom: 1px solid var(--bg-hover);
        min-height: 28px;
    }
    .builder-title {
        font-size: 11px;
        font-weight: 600;
        color: var(--text);
    }
    .toolbar-btn {
        background: var(--bg-hover);
        border: none;
        border-radius: 4px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
        padding: 2px 8px;
        transition: background 0.1s, color 0.1s;
    }
    .toolbar-btn:hover {
        background: var(--bg-active);
        color: var(--text);
    }
    .spacer {
        flex: 1;
    }
    .canvas-wrapper {
        position: relative;
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
    }
    .preview-dock {
        position: absolute;
        bottom: 0;
        right: 0;
        z-index: 10;
        /* Catch hover so the close button can fade in, but keep the
         * preview image itself click-through — the node graph under
         * that rectangle stays draggable. */
    }
    .preview-dock :global(.brush-preview) {
        pointer-events: none;
    }
    .resize-handle {
        /* Top-left inward corner — grabs here grow the preview toward
         * the upper-left since the dock is anchored bottom-right. */
        position: absolute;
        top: 0;
        left: 0;
        width: 14px;
        height: 14px;
        cursor: nwse-resize;
        /* Subtle glyph revealed on dock hover, matching the close-btn
         * "chromeless until needed" pattern. */
        background:
            linear-gradient(
                135deg,
                transparent 0%,
                transparent 45%,
                var(--text-muted) 45%,
                var(--text-muted) 55%,
                transparent 55%,
                transparent 100%
            );
        opacity: 0;
        transition: opacity 0.15s;
    }
    .preview-dock:hover .resize-handle {
        opacity: 0.7;
    }
    .resize-handle:hover {
        opacity: 1 !important;
    }
    .close-btn {
        /* Top-left inward corner with equal h/v padding. Renders on top
         * of the resize handle (same corner) — the handle still has a
         * graspable L-ring exposed around the button's outer edge, and
         * clicks inside the button area dismiss the preview. */
        position: absolute;
        top: 6px;
        left: 6px;
        width: 28px;
        height: 28px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: color-mix(in srgb, var(--bg) 60%, transparent);
        border: none;
        border-radius: 4px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 13px;
        /* Revealed on hover of the whole dock — matches the "chromeless
         * until you need it" pattern used elsewhere in the editor. */
        opacity: 0;
        transition: opacity 0.15s, color 0.15s, background 0.15s;
    }
    .preview-dock:hover .close-btn {
        opacity: 1;
    }
    .close-btn:hover {
        color: var(--text);
        background: var(--bg-active);
    }
    .bookmark {
        /* Small vertical tab sticking out from the right edge — the
         * minimal affordance for "the preview is here, click to pull
         * it out." */
        writing-mode: vertical-rl;
        padding: 12px 6px;
        background: color-mix(in srgb, var(--bg-hover) 80%, transparent);
        border: none;
        border-radius: 4px 0 0 0;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
        letter-spacing: 1px;
        text-transform: uppercase;
        transition: color 0.15s, background 0.15s;
    }
    .bookmark:hover {
        color: var(--text);
        background: var(--bg-hover);
    }
    .fullscreen-btn {
        position: absolute;
        top: 8px;
        right: 8px;
        width: 28px;
        height: 28px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: color-mix(in srgb, var(--bg) 40%, transparent);
        border: none;
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        font-size: 12px;
        z-index: 10;
        transition: background 0.15s, color 0.15s;
    }
    .fullscreen-btn:hover {
        background: var(--accent);
        color: var(--text);
    }
    .brush-builder.fullscreen {
        position: fixed;
        top: 0;
        left: 0;
        width: 100vw;
        height: 100vh;
        z-index: 9999;
    }
</style>
