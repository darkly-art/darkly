<script lang="ts">
    import { onMount } from 'svelte';
    import { initEditor } from './editor';
    import type { DarklyHandle } from '../wasm/pkg/darkly_wasm';

    let canvas: HTMLCanvasElement;
    let handle: DarklyHandle | null = null;
    let paintLayerId: bigint | null = null;
    let isMouseDown = false;

    const CANVAS_WIDTH = 1920;
    const CANVAS_HEIGHT = 1080;
    const BRUSH_RADIUS = 32;
    const BRUSH_COLOR = { r: 220, g: 180, b: 60, a: 200 };

    onMount(async () => {
        canvas.width = CANVAS_WIDTH;
        canvas.height = CANVAS_HEIGHT;

        try {
            handle = await initEditor(canvas);

            // Demo setup: gradient background + noise filter + paint layer
            const bg = handle.add_raster_layer();
            handle.fill_gradient(bg);

            handle.add_filter_layer("noise", { amount: 0.3, resolution: 2 });

            paintLayerId = handle.add_raster_layer();

            // Start render loop
            requestAnimationFrame(renderLoop);
        } catch (e) {
            console.error("Failed to initialize Darkly:", e);
        }
    });

    function renderLoop() {
        if (handle) {
            const t0 = performance.now();
            handle.render();
            const dt = performance.now() - t0;
            if (dt > 2) {
                console.log(`[frame] render: ${dt.toFixed(1)}ms`);
            }
        }
        requestAnimationFrame(renderLoop);
    }

    function getCanvasPos(e: MouseEvent): { x: number; y: number } {
        const rect = canvas.getBoundingClientRect();
        const scaleX = CANVAS_WIDTH / rect.width;
        const scaleY = CANVAS_HEIGHT / rect.height;
        return {
            x: (e.clientX - rect.left) * scaleX,
            y: (e.clientY - rect.top) * scaleY,
        };
    }

    function onMouseDown(e: MouseEvent) {
        isMouseDown = true;
        if (handle && paintLayerId !== null) {
            handle.snapshot(paintLayerId);
            const pos = getCanvasPos(e);
            handle.paint(
                paintLayerId, pos.x, pos.y, BRUSH_RADIUS,
                BRUSH_COLOR.r, BRUSH_COLOR.g, BRUSH_COLOR.b, BRUSH_COLOR.a
            );
        }
    }

    function onMouseMove(e: MouseEvent) {
        if (!isMouseDown || !handle || paintLayerId === null) return;
        const pos = getCanvasPos(e);
        handle.paint(
            paintLayerId, pos.x, pos.y, BRUSH_RADIUS,
            BRUSH_COLOR.r, BRUSH_COLOR.g, BRUSH_COLOR.b, BRUSH_COLOR.a
        );
    }

    function onMouseUp() {
        if (isMouseDown && handle) {
            handle.commit();
        }
        isMouseDown = false;
    }

    function onKeyDown(e: KeyboardEvent) {
        if (!handle) return;
        if (e.ctrlKey && e.key === 'z') {
            e.preventDefault();
            if (e.shiftKey) {
                handle.redo();
            } else {
                handle.undo();
            }
        }
    }
</script>

<svelte:window on:keydown={onKeyDown} />

<div class="container">
    <canvas
        bind:this={canvas}
        on:mousedown={onMouseDown}
        on:mousemove={onMouseMove}
        on:mouseup={onMouseUp}
        on:mouseleave={onMouseUp}
    ></canvas>
</div>

<style>
    :global(body) {
        margin: 0;
        padding: 0;
        background: #111;
        overflow: hidden;
    }

    .container {
        display: flex;
        justify-content: center;
        align-items: center;
        width: 100vw;
        height: 100vh;
    }

    canvas {
        max-width: 100vw;
        max-height: 100vh;
        object-fit: contain;
        cursor: crosshair;
    }
</style>
