<script lang="ts">
    import { onMount } from "svelte";
    import { initEditor } from "./editor";
    import type { DarklyHandle } from "../wasm/pkg/darkly_wasm.js";

    let canvas: HTMLCanvasElement;
    let handle: DarklyHandle | null = null;
    let paintLayerId: bigint | number = 0;
    let painting = false;

    onMount(async () => {
        canvas.width = 1920;
        canvas.height = 1080;

        try {
            handle = await initEditor(canvas);
            console.log("Darkly initialized");

            // Demo setup: gradient bg → blur filter → paint layer
            const bg = handle.add_raster_layer();
            handle.fill_gradient(bg);
            handle.add_filter_layer(0, 8.0); // Gaussian blur, radius 8
            paintLayerId = handle.add_raster_layer();

            requestAnimationFrame(renderLoop);
        } catch (e) {
            console.error("Failed to initialize Darkly:", e);
        }
    });

    function renderLoop() {
        if (handle && handle.needs_render()) {
            handle.render();
        }
        requestAnimationFrame(renderLoop);
    }

    function getCanvasPos(e: MouseEvent): [number, number] {
        const rect = canvas.getBoundingClientRect();
        const x = (e.clientX - rect.left) * (canvas.width / rect.width);
        const y = (e.clientY - rect.top) * (canvas.height / rect.height);
        return [x, y];
    }

    function onPointerDown(e: PointerEvent) {
        if (!handle) return;
        painting = true;
        canvas.setPointerCapture(e.pointerId);
        handle.snapshot();
        const [x, y] = getCanvasPos(e);
        handle.paint(paintLayerId as any, x, y, 8.0, 220, 220, 255, 200);
    }

    function onPointerMove(e: PointerEvent) {
        if (!handle || !painting) return;
        const [x, y] = getCanvasPos(e);
        handle.paint(paintLayerId as any, x, y, 8.0, 220, 220, 255, 200);
    }

    function onPointerUp(_e: PointerEvent) {
        painting = false;
    }

    function onKeyDown(e: KeyboardEvent) {
        if (!handle) return;
        if (e.ctrlKey && e.key === "z") {
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

<canvas
    bind:this={canvas}
    style="width: 100%; height: 100%; display: block; cursor: crosshair;"
    on:pointerdown={onPointerDown}
    on:pointermove={onPointerMove}
    on:pointerup={onPointerUp}
></canvas>
