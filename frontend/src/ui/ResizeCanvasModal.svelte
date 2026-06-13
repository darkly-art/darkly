<script lang="ts">
    import Modal from './Modal.svelte';
    import Icon from '../icons/Icon.svelte';
    import { resizeCanvas } from '../state/resizeCanvas.svelte';
    import { app } from '../state/app.svelte';

    // Current canvas dimensions, captured when the modal opens. `oldW`/`oldH`
    // drive the anchor preview (where the existing content sits inside the
    // resized canvas).
    let oldW = $state(1);
    let oldH = $state(1);
    let width = $state(1);
    let height = $state(1);
    let linkAspect = $state(false);
    // 9-point anchor: each axis in {0, 0.5, 1} — the fraction of the size
    // delta removed from the top/left edge.
    let anchorX = $state(0.5);
    let anchorY = $state(0.5);

    const MAX_DIM = 8192;

    let prevOpen = false;
    $effect(() => {
        if (resizeCanvas.open && !prevOpen) {
            const r = app.handle?.canvas_rect();
            if (r) {
                oldW = r[2];
                oldH = r[3];
                width = r[2];
                height = r[3];
            }
            anchorX = 0.5;
            anchorY = 0.5;
            linkAspect = false;
        }
        prevOpen = resizeCanvas.open;
    });

    function clampDim(v: number): number {
        return Math.max(1, Math.min(MAX_DIM, Math.round(v)));
    }

    function onWidthInput() {
        if (linkAspect && oldW > 0) {
            height = clampDim(width * (oldH / oldW));
        }
    }
    function onHeightInput() {
        if (linkAspect && oldH > 0) {
            width = clampDim(height * (oldW / oldH));
        }
    }

    function setAnchor(ax: number, ay: number) {
        anchorX = ax;
        anchorY = ay;
    }

    function close() {
        resizeCanvas.open = false;
    }

    function apply() {
        const w = clampDim(width);
        const h = clampDim(height);
        app.handle?.resize_canvas(w, h, anchorX, anchorY);
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

    // --- Box-diagram preview ---------------------------------------------
    // Draw the new-canvas outline and the current-content rectangle, the
    // latter positioned inside the new canvas per the chosen anchor. Both are
    // fit into the preview box's union bounding box so grow and shrink read
    // correctly.
    const PREVIEW = 180;
    type Box = { x: number; y: number; w: number; h: number };
    let newBox = $state<Box>({ x: 0, y: 0, w: 0, h: 0 });
    let contentBox = $state<Box>({ x: 0, y: 0, w: 0, h: 0 });

    $effect(() => {
        const w = clampDim(width);
        const h = clampDim(height);
        // Content offset within the new canvas (may be negative when shrinking).
        const cx = (w - oldW) * anchorX;
        const cy = (h - oldH) * anchorY;
        // Union bounding box of the new canvas (0,0,w,h) and the content rect.
        const minX = Math.min(0, cx);
        const minY = Math.min(0, cy);
        const maxX = Math.max(w, cx + oldW);
        const maxY = Math.max(h, cy + oldH);
        const unionW = Math.max(1, maxX - minX);
        const unionH = Math.max(1, maxY - minY);
        const scale = Math.min(PREVIEW / unionW, PREVIEW / unionH);
        const ox = (PREVIEW - unionW * scale) / 2;
        const oy = (PREVIEW - unionH * scale) / 2;
        const map = (bx: number, by: number, bw: number, bh: number): Box => ({
            x: ox + (bx - minX) * scale,
            y: oy + (by - minY) * scale,
            w: bw * scale,
            h: bh * scale,
        });
        newBox = map(0, 0, w, h);
        contentBox = map(cx, cy, oldW, oldH);
    });

    const ANCHORS = [0, 0.5, 1];
</script>

<Modal bind:open={resizeCanvas.open} title="Canvas Size" size="sm">
    <div class="body" onkeydown={onKeydown} role="presentation">
        <div class="row dim-row">
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

        <div class="anchor-preview">
            <div class="anchor">
                <span class="label">Anchor</span>
                <div class="grid">
                    {#each ANCHORS as ay}
                        {#each ANCHORS as ax}
                            <button
                                type="button"
                                class="cell"
                                class:active={anchorX === ax && anchorY === ay}
                                aria-label={`Anchor ${ax},${ay}`}
                                onclick={() => setAnchor(ax, ay)}
                            ></button>
                        {/each}
                    {/each}
                </div>
            </div>

            <div class="preview">
                <span class="label">Preview</span>
                <svg width={PREVIEW} height={PREVIEW} viewBox={`0 0 ${PREVIEW} ${PREVIEW}`}>
                    <rect class="new-canvas" x={newBox.x} y={newBox.y} width={newBox.w} height={newBox.h} />
                    <rect class="content" x={contentBox.x} y={contentBox.y} width={contentBox.w} height={contentBox.h} />
                </svg>
            </div>
        </div>

        <div class="actions">
            <div class="spacer"></div>
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
        min-width: 360px;
    }

    .row {
        display: flex;
        flex-direction: column;
        gap: 6px;
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

    .anchor-preview {
        display: grid;
        grid-template-columns: auto 1fr;
        gap: 18px;
        align-items: start;
    }

    .anchor,
    .preview {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(3, 28px);
        grid-template-rows: repeat(3, 28px);
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

    .preview svg {
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
    }

    .preview .new-canvas {
        fill: var(--bg-hover);
        stroke: var(--accent);
        stroke-width: 1.5;
    }

    .preview .content {
        fill: color-mix(in srgb, var(--accent) 35%, transparent);
        stroke: var(--accent);
        stroke-width: 1;
        stroke-dasharray: 4 3;
    }

    .actions {
        display: flex;
        align-items: center;
        gap: 8px;
        margin-top: 4px;
    }

    .actions .spacer {
        flex: 1;
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
