<script lang="ts">
    import { onMount } from 'svelte';
    import { initEditor } from '../editor';
    import { config } from '../config/store.svelte';
    import { app } from '../state/app.svelte';
    import { nav } from './navigation.svelte';
    import { toolRegistry } from '../tools/registry';
    import type { ToolContext } from '../tools/registry';
    import { screenToCanvas } from './coordinates';
    import ToolOverlay from './ToolOverlay.svelte';
    import { toast } from '../state/toast.svelte';

    let canvas = $state<HTMLCanvasElement>(undefined!);

    // Deferred to avoid re-entering the WASM handle while it's borrowed
    // (ResizeObserver can fire synchronously during layout, mid-render).
    let resizePending = false;
    function syncCanvasSize() {
        if (resizePending) return;
        resizePending = true;
        requestAnimationFrame(() => {
            resizePending = false;
            if (!canvas) return;
            const dpr = window.devicePixelRatio || 1;
            const rect = canvas.getBoundingClientRect();
            const w = Math.round(rect.width * dpr);
            const h = Math.round(rect.height * dpr);
            if (w < 1 || h < 1) return;
            if (canvas.width !== w || canvas.height !== h) {
                canvas.width = w;
                canvas.height = h;
                app.handle?.resize(w, h);
                app.requestFrame();
            }
        });
    }

    onMount(async () => {
        // Size canvas buffer to match its CSS layout
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        canvas.width = Math.round(rect.width * dpr);
        canvas.height = Math.round(rect.height * dpr);

        try {
            const handle = await initEditor(canvas);
            handle.resize(canvas.width, canvas.height);
            app.handle = handle;

            // Demo setup: gradient background + paint layer in a group
            const bg = handle.add_raster_layer();
            handle.fill_gradient(bg);

            const groupId = handle.add_group();
            const paintLayerId = handle.add_raster_layer_in(groupId);
            app.activeLayerId = paintLayerId;

            // Observe element resizes to keep GPU surface in sync
            const ro = new ResizeObserver(() => syncCanvasSize());
            ro.observe(canvas);

            // Fit canvas to view: scale down if needed, but never scale up
            const dprRect = { w: canvas.width, h: canvas.height };
            const docW = config.get('canvas.width') as number;
            const docH = config.get('canvas.height') as number;
            const fitZoom = Math.min(dprRect.w / docW, dprRect.h / docH, 1);
            app.zoom = fitZoom;

            // Kick the first frame
            app.requestFrame();
        } catch (e) {
            console.error("Failed to initialize Darkly:", e);
            toast.show('error', `Failed to initialize: ${e instanceof Error ? e.message : e}`);
        }
    });


    function getToolContext(): ToolContext | null {
        if (!app.handle) return null;
        return {
            handle: app.handle,
            screenToCanvas(sx: number, sy: number) {
                return screenToCanvas(sx, sy, canvas);
            }
        };
    }

    function getCanvasCoords(e: PointerEvent): { x: number; y: number } {
        if (app.handle) {
            return screenToCanvas(e.clientX, e.clientY, canvas);
        }
        // Fallback when no view transform
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        return { x: (e.clientX - rect.left) * dpr, y: (e.clientY - rect.top) * dpr };
    }

    function onPointerDown(e: PointerEvent) {
        // Touch: always capture and track for gesture detection
        if (e.pointerType === 'touch') {
            canvas.setPointerCapture(e.pointerId);
            if (nav.onTouchPointerDown(e)) {
                // Two-finger gesture started — end any in-progress tool stroke
                const ctx = getToolContext();
                if (ctx) {
                    const tool = toolRegistry.get(app.activeToolId);
                    tool?.onPointerUp(ctx, e);
                }
                return;
            }
        }

        // Navigation gets first chance (space+drag)
        if (nav.onPointerDown(e, canvas)) return;

        canvas.setPointerCapture(e.pointerId);

        const ctx = getToolContext();
        if (!ctx) return;
        const pos = getCanvasCoords(e);
        const tool = toolRegistry.get(app.activeToolId);
        tool?.onPointerDown(ctx, e, pos.x, pos.y);
        app.requestFrame();
    }

    function onPointerMove(e: PointerEvent) {
        // Touch gesture: update position and apply gesture transform
        if (e.pointerType === 'touch') {
            nav.onTouchPointerMove(e, canvas);
            if (nav.isTouchGesture) return;
        }

        if (nav.isNavigating) {
            nav.onPointerMove(e, canvas);
            return;
        }

        const ctx = getToolContext();
        if (!ctx) return;
        const pos = getCanvasCoords(e);
        const tool = toolRegistry.get(app.activeToolId);
        tool?.onPointerMove(ctx, e, pos.x, pos.y);
        app.requestFrame();
    }

    function onPointerUp(e: PointerEvent) {
        // Touch: clean up gesture state; skip tool dispatch if gesture occurred
        if (e.pointerType === 'touch') {
            const wasGesture = nav.isTouchGesture;
            nav.onTouchPointerUp(e);
            if (wasGesture) return;
        }

        if (nav.isNavigating) {
            nav.onPointerUp();
            return;
        }

        const ctx = getToolContext();
        if (!ctx) return;
        const tool = toolRegistry.get(app.activeToolId);
        tool?.onPointerUp(ctx, e);
        app.requestFrame();
    }

    const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta']);

    function onKeyDown(e: KeyboardEvent) {
        nav.onKeyDown(e);
        // Don't dismiss overlay for navigation or bare modifier keys
        if (nav.spaceHeld || MODIFIER_KEYS.has(e.key)) return;
        const tool = toolRegistry.get(app.activeToolId);
        if (tool?.onKeyDown?.(e)) return;
        tool?.dismissOverlay?.();
        app.requestFrame();
    }

    // Call onDeactivate/onActivate when the active tool changes.
    let prevToolId = app.activeToolId;
    $effect(() => {
        const id = app.activeToolId;
        if (id !== prevToolId) {
            const ctx = getToolContext();
            if (ctx) {
                toolRegistry.get(prevToolId)?.onDeactivate?.(ctx);
                toolRegistry.get(id)?.onActivate?.(ctx);
            }
            prevToolId = id;
        }
    });

    // Dismiss tool overlay when the active layer changes.
    let prevLayerId = app.activeLayerId;
    $effect(() => {
        const id = app.activeLayerId;
        if (id !== prevLayerId) {
            prevLayerId = id;
            const tool = toolRegistry.get(app.activeToolId);
            tool?.dismissOverlay?.();
        }
    });

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
            app.requestFrame();
        }
    });
</script>

<svelte:window
    onkeydown={onKeyDown}
    onkeyup={(e: KeyboardEvent) => nav.onKeyUp(e)}
/>

<div class="canvas-container">
    <canvas
        bind:this={canvas}
        style:cursor={nav.cursor}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        onwheel={(e: WheelEvent) => { nav.onWheel(e, canvas); app.requestFrame(); }}
    ></canvas>
    {#if canvas}
        <ToolOverlay canvasEl={canvas} />
    {/if}
</div>

<style>
    .canvas-container {
        flex: 1;
        display: flex;
        justify-content: center;
        align-items: center;
        overflow: hidden;
        position: relative;
        min-height: 64px;
        height: 100%;
    }

    canvas {
        width: 100%;
        height: 100%;
        object-fit: contain;
        touch-action: none;
    }
</style>
