<script lang="ts">
    import Modal from './Modal.svelte';
    import Icon from '../icons/Icon.svelte';
    import { resizeCanvas } from '../state/resizeCanvas.svelte';
    import { app } from '../state/app.svelte';
    import {
        type Rect,
        type Handle,
        type Fit,
        HANDLES,
        MAX_DIM,
        clampDim,
        rectFromAnchor,
        matchedAnchor,
        applyDrag,
        computeFit,
        toPreview,
    } from './resizePreview';

    // Content size + plane origin, captured when the modal opens. The whole
    // interaction lives in content space (content occupies [0..oldW]×[0..oldH]);
    // `apply()` lifts the chosen rect to a plane rect with these.
    let oldW = $state(1);
    let oldH = $state(1);
    let originX0 = $state(0);
    let originY0 = $state(0);

    // The new canvas window, in content space. Single source of truth.
    let rect = $state<Rect>({ x: 0, y: 0, w: 1, h: 1 });

    // Numeric inputs (raw, mirrored to/from `rect`).
    let width = $state(1);
    let height = $state(1);
    let linkAspect = $state(false);
    // The anchor used to distribute a numeric size change about the content.
    let anchorX = $state(0.5);
    let anchorY = $state(0.5);

    // Content→preview-pixel fit. Held fixed during a drag (see beginDrag) so the
    // view doesn't rubber-band; recomputed by refit() otherwise.
    let previewW = $state(480);
    const PREVIEW_H = 300;
    let fit = $state<Fit>({ scale: 1, offsetX: 0, offsetY: 0 });
    let dragging = $state(false);

    let canvasEl = $state<HTMLCanvasElement | null>(null);
    // The exported composite, drawn once into an offscreen canvas. Non-reactive
    // ref; `compositeVersion` bumps to trigger a redraw when it lands.
    let compositeCanvas: HTMLCanvasElement | null = null;
    let compositeVersion = $state(0);

    function refit() {
        fit = computeFit(oldW, oldH, rect, Math.max(1, previewW), PREVIEW_H);
    }

    let prevOpen = false;
    $effect(() => {
        if (resizeCanvas.open && !prevOpen) {
            const r = app.handle?.canvas_rect();
            if (r) {
                originX0 = r[0];
                originY0 = r[1];
                oldW = r[2];
                oldH = r[3];
            }
            width = oldW;
            height = oldH;
            anchorX = 0.5;
            anchorY = 0.5;
            linkAspect = false;
            rect = { x: 0, y: 0, w: oldW, h: oldH };
            compositeCanvas = null;
            compositeVersion++;
            refit();
            requestComposite();
        }
        prevOpen = resizeCanvas.open;
    });

    // --- Real-pixel preview source --------------------------------------
    // Reuse the async export readback (offscreen composite → GPU readback →
    // poll). The result is the current canvas window at oldW×oldH, so it maps
    // 1:1 onto the content rect.
    function requestComposite() {
        if (!app.handle) return;
        app.handle.start_export();
        app.onExportResult((result) => {
            if (!resizeCanvas.open) return; // modal closed before it landed
            const cv = document.createElement('canvas');
            cv.width = result.width;
            cv.height = result.height;
            const cctx = cv.getContext('2d');
            if (!cctx) return;
            const clamped = new Uint8ClampedArray(result.rgba.length);
            clamped.set(result.rgba);
            cctx.putImageData(new ImageData(clamped, result.width, result.height), 0, 0);
            compositeCanvas = cv;
            compositeVersion++;
        });
    }

    // --- Numeric / anchor controls --------------------------------------
    function applyDims() {
        rect = rectFromAnchor(oldW, oldH, width, height, anchorX, anchorY);
        refit();
    }
    function onWidthInput() {
        if (linkAspect && oldW > 0) height = clampDim(width * (oldH / oldW));
        applyDims();
    }
    function onHeightInput() {
        if (linkAspect && oldH > 0) width = clampDim(height * (oldW / oldH));
        applyDims();
    }
    function setAnchor(ax: number, ay: number) {
        anchorX = ax;
        anchorY = ay;
        applyDims();
    }

    // Highlight the anchor cell the current rect corresponds to (if any).
    const highlight = $derived(matchedAnchor(oldW, oldH, rect));

    // --- Drag interaction -----------------------------------------------
    function beginDrag(e: PointerEvent, handle: Handle) {
        e.preventDefault();
        const el = e.currentTarget as HTMLElement;
        el.setPointerCapture(e.pointerId);
        const startRect = { ...rect };
        const startFit = fit; // held for the duration of the drag
        const startX = e.clientX;
        const startY = e.clientY;
        dragging = true;
        const onMove = (ev: PointerEvent) => {
            const dx = (ev.clientX - startX) / startFit.scale;
            const dy = (ev.clientY - startY) / startFit.scale;
            rect = applyDrag(startRect, handle, dx, dy, ev.shiftKey);
            width = rect.w;
            height = rect.h;
            // Keep the numeric-distribution anchor consistent when the rect
            // happens to sit on an anchor.
            const m = matchedAnchor(oldW, oldH, rect);
            if (m.ax !== null) anchorX = m.ax;
            if (m.ay !== null) anchorY = m.ay;
        };
        const onUp = (ev: PointerEvent) => {
            dragging = false;
            el.releasePointerCapture?.(ev.pointerId);
            el.removeEventListener('pointermove', onMove);
            el.removeEventListener('pointerup', onUp);
            refit();
        };
        el.addEventListener('pointermove', onMove);
        el.addEventListener('pointerup', onUp);
    }

    // Frame rect in preview (CSS) pixels.
    const frame = $derived.by(() => {
        const [x, y] = toPreview(fit, rect.x, rect.y);
        const [x2, y2] = toPreview(fit, rect.x + rect.w, rect.y + rect.h);
        return { x, y, w: x2 - x, h: y2 - y };
    });

    function handlePos(h: Handle, f: { x: number; y: number; w: number; h: number }) {
        const cx = f.x + f.w / 2;
        const cy = f.y + f.h / 2;
        const x =
            h === 'w' || h === 'nw' || h === 'sw' ? f.x : h === 'n' || h === 's' ? cx : f.x + f.w;
        const y =
            h === 'n' || h === 'nw' || h === 'ne' ? f.y : h === 'e' || h === 'w' ? cy : f.y + f.h;
        return { x, y };
    }
    const CURSORS: Record<Handle, string> = {
        nw: 'nwse-resize',
        se: 'nwse-resize',
        ne: 'nesw-resize',
        sw: 'nesw-resize',
        n: 'ns-resize',
        s: 'ns-resize',
        e: 'ew-resize',
        w: 'ew-resize',
        body: 'move',
    };

    // --- Canvas rendering (composite + dim) -----------------------------
    $effect(() => {
        // Re-run on any of these.
        void compositeVersion;
        const f = fit;
        const r = rect;
        const cv = canvasEl;
        if (!cv) return;
        const ctx = cv.getContext('2d');
        if (!ctx) return;
        const dpr = window.devicePixelRatio || 1;
        const W = Math.max(1, previewW);
        const H = PREVIEW_H;
        const bw = Math.round(W * dpr);
        const bh = Math.round(H * dpr);
        if (cv.width !== bw || cv.height !== bh) {
            cv.width = bw;
            cv.height = bh;
        }
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx.clearRect(0, 0, W, H);

        // Composite at the content rect (alpha preserved; CSS checkerboard shows
        // through transparent regions).
        if (compositeCanvas) {
            const [x0, y0] = toPreview(f, 0, 0);
            const [x1, y1] = toPreview(f, oldW, oldH);
            ctx.imageSmoothingEnabled = true;
            ctx.drawImage(compositeCanvas, x0, y0, x1 - x0, y1 - y0);
        }

        // Dim everything OUTSIDE the new-canvas rect (crop-tool look).
        const [rx, ry] = toPreview(f, r.x, r.y);
        const [rx2, ry2] = toPreview(f, r.x + r.w, r.y + r.h);
        const cx = Math.max(0, Math.min(W, rx));
        const cy = Math.max(0, Math.min(H, ry));
        const cx2 = Math.max(0, Math.min(W, rx2));
        const cy2 = Math.max(0, Math.min(H, ry2));
        ctx.fillStyle = 'rgba(0, 0, 0, 0.5)';
        ctx.fillRect(0, 0, W, cy); // top band
        ctx.fillRect(0, cy2, W, H - cy2); // bottom band
        ctx.fillRect(0, cy, cx, cy2 - cy); // left band
        ctx.fillRect(cx2, cy, W - cx2, cy2 - cy); // right band
    });

    // Refit when the stage resizes (but never mid-drag — the fit is held then).
    $effect(() => {
        void previewW;
        if (!dragging) refit();
    });

    function close() {
        resizeCanvas.open = false;
    }

    function apply() {
        const w = clampDim(rect.w);
        const h = clampDim(rect.h);
        app.handle?.resize_canvas_rect(originX0 + rect.x, originY0 + rect.y, w, h);
        // The new origin/dims are known synchronously in this JS turn, so the
        // coordinate transforms recenter before any pointer event reads them.
        app.syncCanvasRect();
        app.refreshLayerTree();
        app.requestFrame();
        close();
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Enter') {
            e.preventDefault();
            apply();
        }
    }

    const ANCHORS = [0, 0.5, 1];
</script>

<Modal bind:open={resizeCanvas.open} title="Canvas Size" size="md">
    <div class="body" onkeydown={onKeydown} role="presentation">
        <div class="dim-row">
            <label class="field">
                <span class="label">Width</span>
                <div class="num">
                    <input type="number" min="1" max={MAX_DIM} bind:value={width} oninput={onWidthInput} />
                    <span class="unit">px</span>
                </div>
            </label>
            <label class="field">
                <span class="label">Height</span>
                <div class="num">
                    <input type="number" min="1" max={MAX_DIM} bind:value={height} oninput={onHeightInput} />
                    <span class="unit">px</span>
                </div>
            </label>
            <button
                type="button"
                class="link-toggle"
                class:active={linkAspect}
                aria-pressed={linkAspect}
                aria-label={linkAspect ? 'Unlock aspect ratio' : 'Lock aspect ratio'}
                title={linkAspect ? 'Unlock aspect ratio' : 'Lock aspect ratio'}
                onclick={() => (linkAspect = !linkAspect)}
            >
                <Icon name={linkAspect ? 'fa6-solid:link' : 'fa6-solid:link-slash'} />
            </button>
        </div>

        <div class="preview-stage checker" bind:clientWidth={previewW} style={`height:${PREVIEW_H}px`}>
            <canvas bind:this={canvasEl} style={`width:100%;height:${PREVIEW_H}px`}></canvas>
            <svg
                class="overlay"
                class:dragging
                width="100%"
                height={PREVIEW_H}
                viewBox={`0 0 ${Math.max(1, previewW)} ${PREVIEW_H}`}
                preserveAspectRatio="none"
            >
                <!-- Body: drag the whole frame. -->
                <rect
                    class="frame-body"
                    x={frame.x}
                    y={frame.y}
                    width={Math.max(0, frame.w)}
                    height={Math.max(0, frame.h)}
                    style={`cursor:${CURSORS.body}`}
                    onpointerdown={(e) => beginDrag(e, 'body')}
                    role="presentation"
                />
                <rect
                    class="frame-outline"
                    x={frame.x}
                    y={frame.y}
                    width={Math.max(0, frame.w)}
                    height={Math.max(0, frame.h)}
                />
                {#each HANDLES as h}
                    {@const p = handlePos(h, frame)}
                    <rect
                        class="handle"
                        x={p.x - 6}
                        y={p.y - 6}
                        width="12"
                        height="12"
                        style={`cursor:${CURSORS[h]}`}
                        aria-label={`Resize ${h}`}
                        onpointerdown={(e) => beginDrag(e, h)}
                        role="presentation"
                    />
                {/each}
            </svg>
        </div>

        <div class="actions">
            <div class="anchor">
                <span class="label">Anchor</span>
                <div class="grid">
                    {#each ANCHORS as ay}
                        {#each ANCHORS as ax}
                            <button
                                type="button"
                                class="cell"
                                class:active={highlight.ax === ax && highlight.ay === ay}
                                aria-label={`Anchor ${ax},${ay}`}
                                onclick={() => setAnchor(ax, ay)}
                            ></button>
                        {/each}
                    {/each}
                </div>
            </div>
            <div class="spacer"></div>
            <div class="dims-readout">{clampDim(rect.w)} × {clampDim(rect.h)} px</div>
            <button type="button" class="cancel" onclick={close}>Cancel</button>
            <button type="button" class="ok" onclick={apply}>Resize</button>
        </div>
    </div>
</Modal>

<style>
    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 440px;
    }

    .dim-row {
        display: grid;
        grid-template-columns: 1fr 1fr auto;
        gap: 12px;
        align-items: end;
    }

    .field {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .label {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: var(--text-muted);
    }

    .num {
        display: flex;
        align-items: center;
        gap: 4px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 0 8px;
    }

    .num input {
        flex: 1;
        background: transparent;
        border: none;
        color: var(--text);
        padding: 6px 0;
        font: inherit;
        outline: none;
        min-width: 0;
    }

    .num .unit {
        color: var(--text-muted);
        font-family: var(--font-mono, monospace);
        font-size: 12px;
    }

    .link-toggle {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 34px;
        height: 34px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        color: var(--text-muted);
        font-size: 14px;
        cursor: pointer;
    }

    .link-toggle:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .link-toggle.active {
        background: var(--accent);
        border-color: var(--accent);
        color: #fff;
    }

    /* Interactive preview ------------------------------------------------ */
    .preview-stage {
        position: relative;
        width: 100%;
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        overflow: hidden;
        touch-action: none;
    }

    .checker {
        background-color: #1e1e1e;
        background-image:
            linear-gradient(45deg, #2a2a2a 25%, transparent 25%),
            linear-gradient(-45deg, #2a2a2a 25%, transparent 25%),
            linear-gradient(45deg, transparent 75%, #2a2a2a 75%),
            linear-gradient(-45deg, transparent 75%, #2a2a2a 75%);
        background-size: 16px 16px;
        background-position:
            0 0,
            0 8px,
            8px -8px,
            -8px 0;
    }

    .preview-stage canvas {
        position: absolute;
        inset: 0;
        display: block;
    }

    .overlay {
        position: absolute;
        inset: 0;
    }

    .frame-body {
        fill: transparent;
    }

    .frame-outline {
        fill: none;
        stroke: var(--accent);
        stroke-width: 1.5;
        vector-effect: non-scaling-stroke;
        pointer-events: none;
    }

    .handle {
        fill: #fff;
        stroke: var(--accent);
        stroke-width: 1.5;
        vector-effect: non-scaling-stroke;
    }

    .handle:hover {
        fill: var(--accent);
    }

    /* Actions + anchor --------------------------------------------------- */
    .actions {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .anchor {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(3, 20px);
        grid-template-rows: repeat(3, 20px);
        gap: 3px;
    }

    .cell {
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 3px;
        cursor: pointer;
        padding: 0;
    }

    .cell:hover {
        background: var(--bg-hover);
    }

    .cell.active {
        background: var(--accent);
        border-color: var(--accent);
    }

    .spacer {
        flex: 1;
    }

    .dims-readout {
        color: var(--text-muted);
        font-family: var(--font-mono, monospace);
        font-size: 12px;
    }

    .actions button {
        padding: 6px 14px;
        border-radius: 4px;
        border: 1px solid var(--bg-hover);
        background: var(--bg);
        color: var(--text);
        font: inherit;
        cursor: pointer;
    }

    .actions button:hover:not(:disabled) {
        background: var(--bg-hover);
    }

    .actions .ok {
        background: var(--accent);
        border-color: var(--accent);
        color: #fff;
    }

    .actions .ok:hover:not(:disabled) {
        filter: brightness(1.1);
    }
</style>
