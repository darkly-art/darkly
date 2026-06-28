<script lang="ts">
    import Modal from './Modal.svelte';
    import Icon from '../icons/Icon.svelte';
    import { imageRescale } from '../state/imageRescale.svelte';
    import { app } from '../state/app.svelte';
    import { MAX_DIM, clampDim } from './resizePreview';

    // Source dimensions, captured when the modal opens.
    let oldW = $state(1);
    let oldH = $state(1);

    // Target dimensions in pixels — the model. Inputs (`wInput`/`hInput`) are
    // shown in the current `unit` and converted to/from these.
    let pxW = $state(1);
    let pxH = $state(1);

    let unit = $state<'px' | '%'>('px');
    // Aspect-ratio link defaults ON: a non-uniform rescale distorts content.
    let linkAspect = $state(true);

    let wInput = $state(1);
    let hInput = $state(1);

    function fromPxW(px: number): number {
        return unit === 'px' ? px : Math.round((px / oldW) * 1000) / 10;
    }
    function fromPxH(px: number): number {
        return unit === 'px' ? px : Math.round((px / oldH) * 1000) / 10;
    }
    function toPxW(v: number): number {
        return unit === 'px' ? clampDim(v) : clampDim((oldW * v) / 100);
    }
    function toPxH(v: number): number {
        return unit === 'px' ? clampDim(v) : clampDim((oldH * v) / 100);
    }

    // Refresh the displayed inputs from the pixel model (after a unit switch,
    // an aspect-linked change to the other axis, or on open).
    function syncInputs() {
        wInput = fromPxW(pxW);
        hInput = fromPxH(pxH);
    }

    let prevOpen = false;
    $effect(() => {
        if (imageRescale.open && !prevOpen) {
            oldW = app.docW;
            oldH = app.docH;
            pxW = oldW;
            pxH = oldH;
            unit = 'px';
            linkAspect = true;
            syncInputs();
        }
        prevOpen = imageRescale.open;
    });

    function onWidthInput() {
        pxW = toPxW(wInput);
        if (linkAspect) {
            pxH = clampDim((pxW * oldH) / oldW);
        }
        syncInputs();
    }

    function onHeightInput() {
        pxH = toPxH(hInput);
        if (linkAspect) {
            pxW = clampDim((pxH * oldW) / oldH);
        }
        syncInputs();
    }

    function setUnit(u: 'px' | '%') {
        unit = u;
        syncInputs();
    }

    function close() {
        imageRescale.open = false;
    }

    function apply() {
        const w = clampDim(pxW);
        const h = clampDim(pxH);
        app.engine?.post('rescale_image', { new_width: w, new_height: h });
        // New dims are known synchronously this JS turn — recenter the
        // coordinate transforms before any pointer event reads them.
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
</script>

<Modal bind:open={imageRescale.open} title="Image Size" size="sm">
    <div class="body" onkeydown={onKeydown} role="presentation">
        <div class="unit-row">
            <span class="label">Units</span>
            <div class="unit-toggle">
                <button type="button" class:active={unit === 'px'} onclick={() => setUnit('px')}>px</button>
                <button type="button" class:active={unit === '%'} onclick={() => setUnit('%')}>%</button>
            </div>
        </div>

        <div class="dim-row">
            <label class="field">
                <span class="label">Width</span>
                <div class="num">
                    <input
                        type="number"
                        min="1"
                        max={unit === 'px' ? MAX_DIM : undefined}
                        step={unit === 'px' ? 1 : 0.1}
                        bind:value={wInput}
                        oninput={onWidthInput}
                    />
                    <span class="unit">{unit}</span>
                </div>
            </label>
            <label class="field">
                <span class="label">Height</span>
                <div class="num">
                    <input
                        type="number"
                        min="1"
                        max={unit === 'px' ? MAX_DIM : undefined}
                        step={unit === 'px' ? 1 : 0.1}
                        bind:value={hInput}
                        oninput={onHeightInput}
                    />
                    <span class="unit">{unit}</span>
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

        <div class="actions">
            <div class="dims-readout">{clampDim(pxW)} × {clampDim(pxH)} px</div>
            <button type="button" class="cancel" onclick={close}>Cancel</button>
            <button type="button" class="ok" onclick={apply}>Rescale</button>
        </div>
    </div>
</Modal>

<style>
    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 320px;
    }

    .label {
        font-size: 12px;
        color: var(--text-muted);
    }

    .unit-row {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .unit-toggle {
        display: inline-flex;
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        overflow: hidden;
    }

    .unit-toggle button {
        background: transparent;
        border: none;
        color: var(--text-muted);
        padding: 4px 12px;
        cursor: pointer;
        font-size: 12px;
    }

    .unit-toggle button.active {
        background: var(--accent, var(--bg-hover));
        color: var(--text);
    }

    .dim-row {
        display: flex;
        align-items: flex-end;
        gap: 12px;
    }

    .field {
        display: flex;
        flex-direction: column;
        gap: 4px;
        flex: 1;
    }

    .num {
        display: flex;
        align-items: center;
        gap: 6px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 0 8px;
    }

    .num input {
        flex: 1;
        background: transparent;
        border: none;
        color: var(--text);
        padding: 6px 0;
        width: 100%;
        font-size: 14px;
    }

    .num input:focus {
        outline: none;
    }

    .num .unit {
        color: var(--text-muted);
        font-size: 12px;
    }

    .link-toggle {
        background: transparent;
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        color: var(--text-muted);
        cursor: pointer;
        height: 32px;
        width: 32px;
        display: flex;
        align-items: center;
        justify-content: center;
    }

    .link-toggle.active {
        color: var(--text);
        border-color: var(--accent, var(--text-muted));
    }

    .actions {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .dims-readout {
        flex: 1;
        font-size: 12px;
        color: var(--text-muted);
    }

    .cancel,
    .ok {
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 7px 16px;
        cursor: pointer;
        font-size: 13px;
    }

    .cancel {
        background: transparent;
        color: var(--text-muted);
    }

    .ok {
        background: var(--accent, var(--bg-hover));
        color: var(--text);
        border-color: transparent;
    }
</style>
