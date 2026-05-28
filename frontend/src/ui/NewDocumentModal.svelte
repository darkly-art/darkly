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
        }
        prevOpen = newDocument.open;
    });

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
            const clip = await readImageFromClipboard();
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

        <div class="actions">
            <button
                type="button"
                class="clipboard"
                onclick={fromClipboard}
                disabled={clipboardBusy}
            >From Clipboard</button>
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
