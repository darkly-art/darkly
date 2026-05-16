<script lang="ts">
    import Modal from './Modal.svelte';
    import { exportImage } from '../state/exportImage.svelte';
    import { app } from '../state/app.svelte';
    import { downloadBlob, sanitizeFilename } from '../storage';

    type Format = 'png' | 'jpeg' | 'webp';

    const FORMATS: Array<{ id: Format; label: string; mime: string; ext: string }> = [
        { id: 'png',  label: 'PNG',  mime: 'image/png',  ext: 'png'  },
        { id: 'jpeg', label: 'JPEG', mime: 'image/jpeg', ext: 'jpg'  },
        { id: 'webp', label: 'WebP', mime: 'image/webp', ext: 'webp' },
    ];

    // JPEG/WebP quality is fixed at 0.92 (per the plan) — no slider in v1.
    const QUALITY = 0.92;

    let format = $state<Format>('png');
    let baseName = $state('darkly-export');
    let exporting = $state(false);

    function close() {
        if (exporting) return;
        exporting = false;
        exportImage.open = false;
    }

    async function encodeAndDownload(width: number, height: number, rgba: Uint8Array) {
        const fmt = FORMATS.find(f => f.id === format)!;
        const canvas = new OffscreenCanvas(width, height);
        const ctx = canvas.getContext('2d')!;
        // ImageData rejects buffers backed by SharedArrayBuffer (which the
        // WASM heap can be). Copy into a fresh ArrayBuffer first — same
        // pattern as copyToSystemClipboard.
        const copy = new Uint8ClampedArray(rgba.length);
        copy.set(rgba);
        ctx.putImageData(new ImageData(copy, width, height), 0, 0);
        const blob =
            fmt.id === 'png'
                ? await canvas.convertToBlob({ type: fmt.mime })
                : await canvas.convertToBlob({ type: fmt.mime, quality: QUALITY });
        const filename = `${sanitizeFilename(baseName) || 'darkly-export'}.${fmt.ext}`;
        downloadBlob(blob, filename);
    }

    function confirm() {
        if (!app.handle || exporting) return;
        exporting = true;
        const handle = app.handle;
        app.onExportResult(async (result) => {
            try {
                if (result?.rgba) {
                    await encodeAndDownload(result.width, result.height, result.rgba);
                }
            } catch (e) {
                console.error('[export-image] encode failed', e);
                alert('Export failed — see console for details.');
            } finally {
                exporting = false;
                exportImage.open = false;
            }
        });
        handle.start_export();
    }
</script>

<Modal bind:open={exportImage.open} title="Export Image" size="sm">
    <div class="export-body">
        <label class="row">
            <span class="label">Filename</span>
            <div class="filename">
                <input
                    type="text"
                    bind:value={baseName}
                    placeholder="darkly-export"
                    disabled={exporting}
                />
                <span class="ext">.{FORMATS.find(f => f.id === format)!.ext}</span>
            </div>
        </label>

        <label class="row">
            <span class="label">Format</span>
            <select bind:value={format} disabled={exporting}>
                {#each FORMATS as f (f.id)}
                    <option value={f.id}>{f.label}</option>
                {/each}
            </select>
        </label>

        <div class="actions">
            <button type="button" class="cancel" onclick={close} disabled={exporting}>
                Cancel
            </button>
            <button type="button" class="ok" onclick={confirm} disabled={exporting}>
                {exporting ? 'Exporting…' : 'Export'}
            </button>
        </div>
    </div>
</Modal>

<style>
    .export-body {
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

    .label {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: var(--text-muted);
    }

    .filename {
        display: flex;
        align-items: center;
        gap: 4px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 0 8px;
    }

    .filename input {
        flex: 1;
        background: transparent;
        border: none;
        color: var(--text);
        padding: 6px 0;
        font: inherit;
        outline: none;
    }

    .filename .ext {
        color: var(--text-muted);
        font-family: var(--font-mono, monospace);
        font-size: 12px;
    }

    select {
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 6px 8px;
        font: inherit;
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin-top: 4px;
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

    .actions button:disabled {
        opacity: 0.5;
        cursor: default;
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
