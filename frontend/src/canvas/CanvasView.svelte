<script lang="ts">
    import { onMount } from 'svelte';
    import { initEditor, createInstance, ensureProcessInit } from '../editor';
    import { config } from '../config/store.svelte';
    import { app, type DarklyInstance } from '../state/app.svelte';
    import { nav } from './navigation.svelte';
    import { toolRegistry } from '../tools/registry';
    import type { ToolContext } from '../tools/registry';
    import { screenToCanvas } from './coordinates';
    import { toast } from '../state/toast.svelte';
    import { theme } from '../state/theme.svelte';
    import { dispatchDrag } from '../actions/triggers';
    import { handleDroppedFile } from '../actions';
    import { THUMB_SIZE } from '../ui/layers/thumbnails';

    /** Optional pre-built instance. When provided, CanvasView skips the
     *  single-instance bootstrap (`initEditor`) and just wires the canvas
     *  to this instance — the multi-tab shell uses this to render N
     *  CanvasViews, each bound to its own pre-created instance.
     *
     *  When omitted, CanvasView calls `initEditor` and binds to the global
     *  `app` proxy — the existing single-instance behaviour. */
    let { instance: providedInstance = undefined as DarklyInstance | undefined } = $props();

    /** The instance this view is bound to. For multi-tab CanvasViews this is
     *  the prop-supplied instance; for the single-instance host it's `app`
     *  (a Proxy that resolves to the active `DarklyInstance`). All reads of
     *  per-instance state go through this binding so an inactive multi-tab
     *  CanvasView still routes pointer events to its own instance, never to
     *  the focused one. */
    const inst = $derived<DarklyInstance>(providedInstance ?? app);

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
                inst.handle?.resize(w, h);
                // Re-sync the Rust view transform with the new screen dimensions
                // so the compositor and JS coordinate conversion agree.
                const dpr2 = dpr;
                inst.handle?.set_view_transform(
                    inst.panX * dpr2, inst.panY * dpr2,
                    inst.zoom, inst.rotation,
                    w, h,
                );
                inst.requestFrame();
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
            // Whether we should seed a default background layer. Fresh
            // tabs need one; tabs whose handle was pre-built already
            // have the loaded doc's layers and must not get an extra
            // bg layer on top.
            //
            // An `onHandleReady` callback also signals "I'll provide
            // content the moment the handle is alive" — that's the
            // Open flow seeding either an `open_document(bytes)` or a
            // `paste_image(...)` of the picked file. Skipping the bg
            // seed in that case avoids the wasted allocation that
            // `open_document` would immediately replace, and keeps the
            // canvas free of an unwanted "Layer 1" under an opened PNG.
            const seedBackground =
                (!providedInstance || !providedInstance.handle)
                && !providedInstance?.onHandleReady;

            let handle;
            if (providedInstance && providedInstance.handle) {
                // Multi-tab, hot path: shell pre-built handle + canvas.
                handle = providedInstance.handle;
                providedInstance.canvasEl = canvas;
            } else if (providedInstance) {
                // Multi-tab, first-mount path: shell put a fresh instance in
                // the strip but its async handle creation is up to us — the
                // canvas only exists once Svelte mounts it. `config.get`
                // requires WASM+config to be initialised, so prime that
                // first.
                await ensureProcessInit();
                // Per-tab dim override (`shell.open(name, {w,h})`) wins
                // over the global default — the Open flow for images
                // sizes the canvas to the file's intrinsic dimensions.
                const dims = providedInstance.pendingDims;
                const docW = dims?.width ?? (config.get('canvas.width') as number);
                const docH = dims?.height ?? (config.get('canvas.height') as number);
                providedInstance.pendingDims = null;
                await createInstance(canvas, docW, docH, providedInstance, { seedBackground });
                handle = providedInstance.handle!;
            } else {
                // Single-instance path: existing initEditor creates an
                // instance, makes it active, and returns its handle.
                handle = await initEditor(canvas);
            }
            handle.resize(canvas.width, canvas.height);

            // Drift guard: the engine auto-queues thumbnail readbacks
            // at `DEFAULT_THUMB_SIZE`; the panel renders <img> at the
            // TS-side `THUMB_SIZE`. If they fall out of sync, cached
            // bytes won't fit the displayed dimensions. Fail loudly.
            const engineThumb = handle.engine_default_thumb_size();
            if (engineThumb !== THUMB_SIZE) {
                console.error(
                    `Thumbnail size drift: engine=${engineThumb} ts=${THUMB_SIZE}`,
                );
            }

            // Push the initial UI theme colors so preset-thumbnail bakes
            // match the user's current theme from frame one.
            theme.pushToWasm();

            // Observe element resizes to keep GPU surface in sync
            const ro = new ResizeObserver(() => syncCanvasSize());
            ro.observe(canvas);

            // Fit canvas to view: scale down if needed, but never scale up
            const dprRect = { w: canvas.width, h: canvas.height };
            const docW = config.get('canvas.width') as number;
            const docH = config.get('canvas.height') as number;
            const fitZoom = Math.min(dprRect.w / docW, dprRect.h / docH, 1);
            inst.zoom = fitZoom;

            // Kick the first frame
            inst.requestFrame();
        } catch (e) {
            console.error("Failed to initialize Darkly:", e);
            toast.show('error', `Failed to initialize: ${e instanceof Error ? e.message : e}`);
        }
    });


    function getToolContext(): ToolContext | null {
        if (!inst.handle) return null;
        return {
            handle: inst.handle,
            canvasEl: canvas,
            screenToCanvas(sx: number, sy: number) {
                return screenToCanvas(sx, sy, canvas);
            }
        };
    }

    function getCanvasCoords(e: PointerEvent): { x: number; y: number } {
        if (inst.handle) {
            return screenToCanvas(e.clientX, e.clientY, canvas);
        }
        // Fallback when no view transform
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        return { x: (e.clientX - rect.left) * dpr, y: (e.clientY - rect.top) * dpr };
    }

    function onPointerDown(e: PointerEvent) {
        // Prevent browser from synthesising fling/scroll gestures from pen
        // input — touch-action:none only covers touch, not pen (Chromium bug).
        e.preventDefault();

        // Touch: always capture and track for gesture detection
        if (e.pointerType === 'touch') {
            canvas.setPointerCapture(e.pointerId);
            if (nav.onTouchPointerDown(e)) {
                // Touch consumed by navigation — end any in-progress tool stroke
                const ctx = getToolContext();
                if (ctx) {
                    const tool = toolRegistry.get(inst.activeToolId);
                    tool?.onPointerUp(ctx, e);
                }
                return;
            }
        }

        // Navigation gets first chance (space+drag)
        if (nav.onPointerDown(e, canvas)) return;

        const pos = getCanvasCoords(e);
        const ctx = getToolContext();
        const tool = toolRegistry.get(inst.activeToolId);

        // Active tool may claim the pointer before global drag chords
        // (e.g. shift+drag → brush-size scrub) get a shot at it. Used
        // by modal tools whose UI owns the canvas while active.
        const claimed = !!(ctx && tool?.claimsPointer?.(ctx, e, pos.x, pos.y));

        // Drag-bound actions consume the pointer lifecycle before the
        // active tool sees it — unless the tool claimed it.
        if (!claimed && dispatchDrag('canvas', e, { x: pos.x, y: pos.y })) return;

        canvas.setPointerCapture(e.pointerId);

        if (!ctx) return;
        tool?.onPointerDown(ctx, e, pos.x, pos.y);
        inst.requestFrame();
    }

    function onPointerMove(e: PointerEvent) {
        e.preventDefault();

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
        const tool = toolRegistry.get(inst.activeToolId);
        tool?.onPointerMove(ctx, e, pos.x, pos.y);
        inst.requestFrame();
    }

    function onPointerUp(e: PointerEvent) {
        e.preventDefault();

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
        const tool = toolRegistry.get(inst.activeToolId);
        tool?.onPointerUp(ctx, e);
        inst.requestFrame();
    }

    function onPointerCancel(e: PointerEvent) {
        e.preventDefault();

        // Pen/touch can fire pointercancel instead of pointerup (pen lifted
        // out of range, system gesture, browser intervention).  Clean up
        // the same state that onPointerUp would.
        if (e.pointerType === 'touch') {
            nav.onTouchPointerUp(e);
        }
        if (nav.isNavigating) {
            nav.onPointerUp();
            return;
        }
        const ctx = getToolContext();
        if (!ctx) return;
        const tool = toolRegistry.get(inst.activeToolId);
        tool?.onPointerUp(ctx, e);
        inst.requestFrame();
    }

    function onPointerLeave() {
        const ctx = getToolContext();
        if (!ctx) return;
        const tool = toolRegistry.get(inst.activeToolId);
        tool?.onPointerLeave?.(ctx);
        inst.requestFrame();
    }

    // `dragover` MUST preventDefault for a subsequent `drop` to fire —
    // browser default is "block the drop, fall back to navigation".
    function onCanvasDragOver(e: DragEvent) {
        if (!e.dataTransfer?.types?.includes('Files')) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
    }

    // Single-file drop. Routes by content: `.darkly` opens as a new
    // tab (mirrors the Open action), image pastes as a layer in the
    // current tab (the gesture says "I want this here"). Multi-file
    // is intentionally not supported in v1 — too many ambiguous
    // semantics (open all? merge into one doc? layer-import all?).
    function onCanvasDrop(e: DragEvent) {
        const file = e.dataTransfer?.files?.[0];
        if (!file) return;
        e.preventDefault();
        void handleDroppedFile(file);
    }

    const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta']);

    function onKeyDown(e: KeyboardEvent) {
        nav.onKeyDown(e);
        // Don't dismiss overlay for navigation or bare modifier keys
        if (nav.spaceHeld || MODIFIER_KEYS.has(e.key)) return;
        const tool = toolRegistry.get(inst.activeToolId);
        if (tool?.onKeyDown?.(e)) return;
        tool?.dismissOverlay?.();
        inst.requestFrame();
    }

    // Call onDeactivate/onActivate when the active tool changes.
    let prevToolId = '';
    $effect(() => {
        const id = inst.activeToolId;
        if (id !== prevToolId) {
            const ctx = getToolContext();
            if (ctx) {
                // Reset before deactivate so a tool's onDeactivate can still
                // override; whatever the new tool's onActivate sets wins.
                inst.toolCursor = null;
                toolRegistry.get(prevToolId)?.onDeactivate?.(ctx);
                toolRegistry.get(id)?.onActivate?.(ctx);
                prevToolId = id;
            }
        }
    });

    // Dismiss tool overlay when the active layer changes.
    let prevLayerId = inst.activeLayerId;
    $effect(() => {
        const id = inst.activeLayerId;
        if (id !== prevLayerId) {
            prevLayerId = id;
            const tool = toolRegistry.get(inst.activeToolId);
            tool?.dismissOverlay?.();
        }
    });

    // Sync view transform whenever pan/zoom/rotation changes.
    // Pan is stored in CSS pixels; the shader operates in buffer pixels.
    // Scale pan by DPR to convert to buffer space.
    $effect(() => {
        if (inst.handle && canvas) {
            const dpr = window.devicePixelRatio || 1;
            inst.handle.set_view_transform(
                inst.panX * dpr, inst.panY * dpr,
                inst.zoom, inst.rotation,
                canvas.width, canvas.height,
            );
            inst.requestFrame();
        }
    });

    // HMR'ing this component re-runs onMount and creates a second WASM
    // engine, wiping the running undo stack. Force a full reload instead.
    if (import.meta.hot) {
        import.meta.hot.accept(() => import.meta.hot!.invalidate());
    }
</script>

<svelte:window
    onkeydown={onKeyDown}
    onkeyup={(e: KeyboardEvent) => nav.onKeyUp(e)}
/>

<div class="canvas-container">
    <canvas
        bind:this={canvas}
        style:cursor={inst.toolCursor ?? nav.cursor}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        onpointercancel={onPointerCancel}
        onpointerleave={onPointerLeave}
        ondragover={onCanvasDragOver}
        ondrop={onCanvasDrop}
        onwheel={(e: WheelEvent) => { nav.onWheel(e, canvas); inst.requestFrame(); }}
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
        min-height: 64px;
        height: 100%;
        background: var(--canvas-bg);
    }

    canvas {
        width: 100%;
        height: 100%;
        object-fit: contain;
        touch-action: none;
    }
</style>
