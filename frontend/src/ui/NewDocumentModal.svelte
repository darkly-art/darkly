<script lang="ts">
    import Modal from './Modal.svelte';
    import { newDocument } from '../state/newDocument.svelte';
    import { shell } from '../multi_tab/shell.svelte';
    import { config } from '../config/store.svelte';
    import { app } from '../state/app.svelte';
    import { readImageFromClipboard } from '../clipboard';
    import { hexToColor } from '../lib/color';
    import ColorInput from './settings/widgets/ColorInput.svelte';

    // Seeded from the global canvas defaults each time the modal opens, so the
    // artist always starts from "what a fresh tab would normally be" but can
    // override per document. Black bg matches the spec.
    let width = $state(1920);
    let height = $state(1080);
    let color = $state('#000000');

    // Auto DPI is the default because it is what makes a brush behave the
    // same on every document: the DPI is derived from the pixel size so the
    // artwork has a consistent physical size. Unchecking means "I know the
    // DPI I want", which for print is 300, so the field is prefilled from
    // `canvas.dpi` rather than from the derived value (an artist who wanted
    // the derived value would have left the checkbox alone).
    let autoDpi = $state(true);
    let dpi = $state(300);

    // Reseed dimensions whenever the modal transitions to open. Reading
    // `config.get` requires the WASM module to be initialised: the modal is
    // gated on an artist click well after boot, so by the time the artist can open
    // it, config has already loaded.
    let prevOpen = false;
    $effect(() => {
        if (newDocument.open && !prevOpen) {
            const w = config.get('canvas.width') as number | undefined;
            const h = config.get('canvas.height') as number | undefined;
            const d = config.get('canvas.dpi') as number | undefined;
            if (typeof w === 'number') width = w;
            if (typeof h === 'number') height = h;
            if (typeof d === 'number') dpi = d;
            autoDpi = true;
            color = '#000000';
            loadClipboardPeek();
        } else if (!newDocument.open && prevOpen) {
            clearClipboardPeek();
        }
        prevOpen = newDocument.open;
    });

    // Clipboard peek: populated when the modal opens, surfaces the dimensions
    // and a thumbnail above the "From Clipboard" button so the artist can see
    // what they're about to paste before committing. Silently empty when no
    // image is on the clipboard or permission is denied; the button still
    // works as a fallback (it reads again on click).
    type ClipboardPeek = {
        rgba: Uint8Array;
        width: number;
        height: number;
        previewUrl: string;
    };
    let clipboardPeek = $state<ClipboardPeek | null>(null);

    async function loadClipboardPeek() {
        clearClipboardPeek();
        const clip = await readImageFromClipboard();
        if (!clip) return;
        // The modal may have closed while the async read was in flight; if so,
        // drop the result on the floor instead of leaking the object URL.
        if (!newDocument.open) return;
        clipboardPeek = {
            rgba: clip.rgba,
            width: clip.width,
            height: clip.height,
            previewUrl: URL.createObjectURL(clip.blob),
        };
    }

    function clearClipboardPeek() {
        if (clipboardPeek) {
            URL.revokeObjectURL(clipboardPeek.previewUrl);
            clipboardPeek = null;
        }
    }

    function close() {
        newDocument.open = false;
    }

    function clampDpi(v: number): number {
        return Number.isFinite(v) ? Math.max(1, Math.min(10000, v)) : 300;
    }

    function create() {
        const w = Math.max(1, Math.min(16384, Math.round(width)));
        const h = Math.max(1, Math.min(16384, Math.round(height)));
        const c = hexToColor(color) ?? { r: 0, g: 0, b: 0, a: 255 };
        const rgba: [number, number, number, number] = [c.r, c.g, c.b, 255];

        // Fresh tab with the chosen canvas size. Setting `onHandleReady`
        // suppresses the default white-image bg seed in CanvasView, leaving
        // us free to seed our own raster layer in the chosen color.
        const inst = shell.open(undefined, {
            width: w,
            height: h,
            dpi: autoDpi ? null : clampDpi(dpi),
        });
        inst.onHandleReady = async (handle) => {
            const bg = await handle.api.addRaster({ anchor: null });
            handle.api.fillBackgroundColor({ id: bg, rgba });
            inst.selectLayer(bg);
            app.refreshLayerTree();
            app.requestFrame();
        };
        close();
    }

    let clipboardBusy = $state(false);

    async function fromClipboard() {
        if (clipboardBusy) return;
        clipboardBusy = true;
        try {
            // Prefer the cached peek so we don't double-read the clipboard
            // (and don't double-trigger a permission prompt). Re-read only
            // when the modal-open peek failed or was skipped.
            const clip = clipboardPeek ?? await readImageFromClipboard();
            if (!clip) return;
            const w = Math.max(1, Math.min(16384, clip.width));
            const h = Math.max(1, Math.min(16384, clip.height));
            // A clipboard-sized document is auto: the artist picked no size,
            // so they picked no DPI either.
            const inst = shell.open(undefined, { width: w, height: h, dpi: null });
            inst.onHandleReady = async (handle) => {
                const { id: bg } = await handle.api.pasteImage(
                    { width: w, height: h, offset_x: 0, offset_y: 0, active_layer_id: -1 },
                    clip.rgba
                );
                inst.selectLayer(bg);
                app.refreshLayerTree();
                app.requestFrame();
            };
            close();
        } finally {
            clipboardBusy = false;
        }
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Enter') {
            e.preventDefault();
            create();
        }
    }
</script>

<Modal bind:open={newDocument.open} title="New Document" size="sm">
    <div class="body" onkeydown={onKeydown} role="presentation">
        <div class="row dim-row">
            <label class="field">
                <span class="field-label">Width</span>
                <div class="field-num">
                    <input
                        type="number"
                        min="1"
                        max="16384"
                        bind:value={width}
                    />
                    <span class="unit">px</span>
                </div>
            </label>
            <label class="field">
                <span class="field-label">Height</span>
                <div class="field-num">
                    <input
                        type="number"
                        min="1"
                        max="16384"
                        bind:value={height}
                    />
                    <span class="unit">px</span>
                </div>
            </label>
        </div>

        <label class="row check-row">
            <input type="checkbox" bind:checked={autoDpi} />
            <span class="field-label">Auto DPI</span>
            <span class="hint">Matches the physical size of the default document</span>
        </label>

        {#if !autoDpi}
            <label class="field dpi-field">
                <span class="field-label">DPI</span>
                <div class="field-num">
                    <input
                        type="number"
                        min="1"
                        max="10000"
                        step="1"
                        bind:value={dpi}
                    />
                    <span class="unit">dpi</span>
                </div>
            </label>
        {/if}

        <label class="row color-row">
            <span class="field-label">Background</span>
            <ColorInput value={color} oninput={(hex) => (color = hex)} onchange={(hex) => (color = hex)} />
        </label>

        {#if clipboardPeek}
            <button
                type="button"
                class="clipboard-preview"
                onclick={fromClipboard}
                disabled={clipboardBusy}
                title="Use this clipboard image"
            >
                <img
                    class="thumb"
                    src={clipboardPeek.previewUrl}
                    alt="Clipboard preview"
                />
                <div class="meta">
                    <span class="field-label">Clipboard image</span>
                    <span class="dim">{clipboardPeek.width} × {clipboardPeek.height} px</span>
                </div>
            </button>
        {/if}

        <div class="dialog-actions">
            <div class="spacer"></div>
            <button type="button" class="btn" onclick={close}>Cancel</button>
            <button type="button" class="btn primary" onclick={create}>Create</button>
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

    .dim-row {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 12px;
    }

    .field-num input::-webkit-inner-spin-button,
    .field-num input::-webkit-outer-spin-button {
        opacity: 0.6;
    }

    .check-row {
        display: flex;
        align-items: center;
        gap: 8px;
        cursor: pointer;
    }

    .check-row .hint {
        color: var(--text-muted);
        font-size: 0.85em;
    }

    /* Half width, so the revealed field lines up with the Width column
       above it rather than stretching across the dialog. */
    .dpi-field {
        width: calc(50% - 6px);
    }

    .clipboard-preview {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 8px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        cursor: pointer;
        text-align: left;
        font: inherit;
        color: var(--text);
        width: 100%;
    }

    .clipboard-preview:hover:not(:disabled) {
        background: var(--bg-hover);
        border-color: var(--accent);
    }

    .clipboard-preview:disabled {
        cursor: default;
        opacity: 0.6;
    }

    .clipboard-preview .thumb {
        width: 64px;
        height: 64px;
        object-fit: contain;
        /* Classic transparency checkerboard so RGBA clipboard images with an
           alpha channel read as transparent, not as black. */
        background-color: #fff;
        background-image:
            linear-gradient(45deg, #ccc 25%, transparent 25%),
            linear-gradient(-45deg, #ccc 25%, transparent 25%),
            linear-gradient(45deg, transparent 75%, #ccc 75%),
            linear-gradient(-45deg, transparent 75%, #ccc 75%);
        background-size: 12px 12px;
        background-position: 0 0, 0 6px, 6px -6px, -6px 0;
        border-radius: 3px;
        flex-shrink: 0;
        image-rendering: pixelated;
    }

    .clipboard-preview .meta {
        display: flex;
        flex-direction: column;
        gap: 4px;
        min-width: 0;
    }

    .clipboard-preview .meta .field-label {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: var(--text-muted);
    }

    .clipboard-preview .meta .dim {
        font-family: var(--font-mono, monospace);
        font-size: 12px;
        color: var(--text);
    }

</style>
