<script lang="ts">
    import Modal from './Modal.svelte';
    import { newDocument } from '../state/newDocument.svelte';
    import { shell } from '../multi_tab/shell.svelte';
    import { config } from '../config/store.svelte';
    import { app } from '../state/app.svelte';
    import { readImageFromClipboard } from '../clipboard';

    // Seeded from the global canvas defaults each time the modal opens, so the
    // user always starts from "what a fresh tab would normally be" but can
    // override per document. Black bg matches the spec.
    let width = $state(1920);
    let height = $state(1080);
    let color = $state('#000000');

    // Reseed dimensions whenever the modal transitions to open. Reading
    // `config.get` requires the WASM module to be initialised — the modal is
    // gated on a user click well after boot, so by the time the user can open
    // it, config has already loaded.
    let prevOpen = false;
    $effect(() => {
        if (newDocument.open && !prevOpen) {
            const w = config.get('canvas.width') as number | undefined;
            const h = config.get('canvas.height') as number | undefined;
            if (typeof w === 'number') width = w;
            if (typeof h === 'number') height = h;
            color = '#000000';
            loadClipboardPeek();
        } else if (!newDocument.open && prevOpen) {
            clearClipboardPeek();
        }
        prevOpen = newDocument.open;
    });

    // Clipboard peek — populated when the modal opens, surfaces the dimensions
    // and a thumbnail above the "From Clipboard" button so the user can see
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

    function parseHex(hex: string): [number, number, number, number] {
        const h = hex.replace('#', '');
        const r = parseInt(h.slice(0, 2), 16);
        const g = parseInt(h.slice(2, 4), 16);
        const b = parseInt(h.slice(4, 6), 16);
        return [r, g, b, 255];
    }

    function create() {
        const w = Math.max(1, Math.min(16384, Math.round(width)));
        const h = Math.max(1, Math.min(16384, Math.round(height)));
        const rgba = parseHex(color);

        // Fresh tab with the chosen canvas size. Setting `onHandleReady`
        // suppresses the default white-image bg seed in CanvasView, leaving
        // us free to seed our own raster layer in the chosen color.
        const inst = shell.open(undefined, { width: w, height: h });
        inst.onHandleReady = (handle) => {
            const bg = handle.add_raster_layer(-1);
            handle.fill_background_color(bg, new Uint8Array(rgba));
            inst.activeLayerId = bg;
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
            const inst = shell.open(undefined, { width: w, height: h });
            inst.onHandleReady = (handle) => {
                const bg = handle.paste_image(w, h, clip.rgba, 0, 0, -1);
                inst.activeLayerId = bg;
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
                <span class="label">Width</span>
                <div class="num">
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
                <span class="label">Height</span>
                <div class="num">
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

        <label class="row color-row">
            <span class="label">Background</span>
            <div class="color">
                <input type="color" bind:value={color} />
                <span class="hex">{color.toUpperCase()}</span>
            </div>
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
                    <span class="label">Clipboard image</span>
                    <span class="dim">{clipboardPeek.width} × {clipboardPeek.height} px</span>
                </div>
            </button>
        {/if}

        <div class="actions">
            <div class="spacer"></div>
            <button type="button" class="cancel" onclick={close}>Cancel</button>
            <button type="button" class="ok" onclick={create}>Create</button>
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

    .row {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .dim-row {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 12px;
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

    .num input::-webkit-inner-spin-button,
    .num input::-webkit-outer-spin-button {
        opacity: 0.6;
    }

    .num .unit {
        color: var(--text-muted);
        font-family: var(--font-mono, monospace);
        font-size: 12px;
    }

    .color {
        display: flex;
        align-items: center;
        gap: 10px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 4px 8px;
    }

    .color input[type="color"] {
        width: 36px;
        height: 24px;
        border: 1px solid var(--bg-hover);
        border-radius: 3px;
        background: transparent;
        padding: 0;
        cursor: pointer;
    }

    .color .hex {
        color: var(--text-muted);
        font-family: var(--font-mono, monospace);
        font-size: 12px;
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

    .clipboard-preview .meta .label {
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
        background: var(--accent);
        filter: brightness(1.1);
    }
</style>
