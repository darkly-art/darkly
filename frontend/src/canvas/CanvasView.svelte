<script lang="ts">
    import { onMount } from 'svelte';
    import { initEditor, DOC_WIDTH, DOC_HEIGHT } from '../editor';
    import { app } from '../state/app.svelte';
    import { nav } from './navigation.svelte';
    import { toolRegistry } from '../tools/registry';
    import type { ToolContext } from '../tools/registry';

    let canvas: HTMLCanvasElement;

    function syncCanvasSize() {
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        const w = Math.round(rect.width * dpr);
        const h = Math.round(rect.height * dpr);
        if (canvas.width !== w || canvas.height !== h) {
            canvas.width = w;
            canvas.height = h;
            app.handle?.resize(w, h);
        }
    }

    onMount(async () => {
        // Size canvas buffer to match its CSS layout
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        canvas.width = Math.round(rect.width * dpr);
        canvas.height = Math.round(rect.height * dpr);

        try {
            const handle = await initEditor(canvas);
            app.handle = handle;

            // Demo setup: gradient background + noise filter + paint layer
            const bg = handle.add_raster_layer();
            handle.fill_gradient(bg);

            handle.add_filter_layer("noise", { amount: 0.3, resolution: 2 });

            const paintLayerId = handle.add_raster_layer();
            app.activeLayerId = Number(paintLayerId);

            // Observe element resizes to keep GPU surface in sync
            const ro = new ResizeObserver(() => syncCanvasSize());
            ro.observe(canvas);

            // Fit canvas to view: scale down if needed, but never scale up
            const dprRect = { w: canvas.width, h: canvas.height };
            const fitZoom = Math.min(dprRect.w / DOC_WIDTH, dprRect.h / DOC_HEIGHT, 1);
            app.zoom = fitZoom;

            // Start render loop
            requestAnimationFrame(renderLoop);
        } catch (e) {
            console.error("Failed to initialize Darkly:", e);
        }
    });

    function renderLoop() {
        if (app.handle) {
            app.handle.render();
        }
        requestAnimationFrame(renderLoop);
    }

    /**
     * Convert CSS-local coordinates (relative to canvas element) to buffer
     * pixel coordinates. Since the canvas buffer matches the element size
     * at device pixel ratio, just scale by DPR.
     */
    function cssToBuffer(cssLocalX: number, cssLocalY: number) {
        const dpr = window.devicePixelRatio || 1;
        return { x: cssLocalX * dpr, y: cssLocalY * dpr };
    }

    function getToolContext(): ToolContext | null {
        if (!app.handle) return null;
        return {
            handle: app.handle,
            screenToCanvas(screenX: number, screenY: number) {
                const rect = canvas.getBoundingClientRect();
                const buf = cssToBuffer(screenX - rect.left, screenY - rect.top);
                const result = app.handle!.screen_to_canvas(buf.x, buf.y);
                return { x: result[0], y: result[1] };
            }
        };
    }

    function getCanvasCoords(e: PointerEvent): { x: number; y: number } {
        const rect = canvas.getBoundingClientRect();
        const buf = cssToBuffer(e.clientX - rect.left, e.clientY - rect.top);
        if (app.handle) {
            const result = app.handle.screen_to_canvas(buf.x, buf.y);
            return { x: result[0], y: result[1] };
        }
        // Fallback when no view transform
        return { x: buf.x, y: buf.y };
    }

    function onPointerDown(e: PointerEvent) {
        // Navigation gets first chance
        if (nav.onPointerDown(e, canvas)) return;

        canvas.setPointerCapture(e.pointerId);

        const ctx = getToolContext();
        if (!ctx) return;
        const pos = getCanvasCoords(e);
        const tool = toolRegistry.get(app.activeToolId);
        tool?.onPointerDown(ctx, e, pos.x, pos.y);
    }

    function onPointerMove(e: PointerEvent) {
        if (nav.isNavigating) {
            nav.onPointerMove(e);
            return;
        }

        const ctx = getToolContext();
        if (!ctx) return;
        const pos = getCanvasCoords(e);
        const tool = toolRegistry.get(app.activeToolId);
        tool?.onPointerMove(ctx, e, pos.x, pos.y);
    }

    function onPointerUp(e: PointerEvent) {
        if (nav.isNavigating) {
            nav.onPointerUp();
            return;
        }

        const ctx = getToolContext();
        if (!ctx) return;
        const tool = toolRegistry.get(app.activeToolId);
        tool?.onPointerUp(ctx, e);
    }

    // Sync view transform whenever pan/zoom/rotation changes.
    // Pan is stored in CSS pixels; the shader operates in buffer pixels.
    // Scale pan by DPR to convert to buffer space.
    $effect(() => {
        if (app.handle && canvas) {
            const dpr = window.devicePixelRatio || 1;
            app.handle.set_view_transform(
                app.panX * dpr, app.panY * dpr,
                app.zoom, app.rotation,
                canvas.width, canvas.height,
            );
        }
    });
</script>

<svelte:window
    onkeydown={(e: KeyboardEvent) => nav.onKeyDown(e)}
    onkeyup={(e: KeyboardEvent) => nav.onKeyUp(e)}
/>

<div class="canvas-container">
    <canvas
        bind:this={canvas}
        style:cursor={nav.cursor}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        onwheel={(e: WheelEvent) => nav.onWheel(e, canvas)}
    ></canvas>
</div>

<style>
    .canvas-container {
        flex: 1;
        display: flex;
        justify-content: center;
        align-items: center;
        overflow: hidden;
        position: relative;
        min-height: 0;
        height: 100%;
    }

    canvas {
        width: 100%;
        height: 100%;
        object-fit: contain;
    }
</style>
