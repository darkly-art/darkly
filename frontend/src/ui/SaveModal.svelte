<script lang="ts">
    import Modal from './Modal.svelte';
    import { saveModal } from '../state/saveModal.svelte';
    import { getActiveInstance } from '../state/app.svelte';
    import {
        saveViaDownload,
        SAVE_FORMATS,
        SAVE_FORMAT_ORDER,
        type Format,
    } from '../storage/saveDocument';

    // Shown only in browsers without the FS Access API (Firefox / Safari); the
    // native picker handles filename + type selection everywhere else.
    const LABELS: Record<Format, string> = {
        darkly: 'Darkly Document',
        png: 'PNG',
        jpeg: 'JPEG',
        webp: 'WebP',
    };

    let format = $state<Format>('darkly');
    let baseName = $state('');
    let saving = $state(false);

    // Seed the filename + reset the type each time the modal opens.
    $effect(() => {
        if (saveModal.open) {
            baseName = saveModal.suggestedName;
            format = 'darkly';
        }
    });

    // Any close path (Cancel button, Escape, backdrop, ×) sets `open` false via
    // the Modal binding: resolve the pending `request()` so the awaiting save
    // flow (and the close-guard) unblocks.
    $effect(() => {
        if (!saveModal.open) saveModal.finish();
    });

    async function confirm() {
        const instance = getActiveInstance();
        if (!instance?.engine || saving) return;
        saving = true;
        try {
            await saveViaDownload(instance, format, baseName);
            saveModal.finish();
        } catch (e) {
            console.error('[save] download failed', e);
            alert('Save failed: see console for details.');
        } finally {
            saving = false;
        }
    }
</script>

<Modal bind:open={saveModal.open} title="Save" size="sm">
    <div class="save-body">
        <label class="row">
            <span class="label">Filename</span>
            <div class="filename">
                <input
                    type="text"
                    bind:value={baseName}
                    placeholder="darkly-document"
                    disabled={saving}
                />
                <span class="ext">.{SAVE_FORMATS[format].ext}</span>
            </div>
        </label>

        <label class="row">
            <span class="label">Type</span>
            <select bind:value={format} disabled={saving}>
                {#each SAVE_FORMAT_ORDER as f (f)}
                    <option value={f}>{LABELS[f]}</option>
                {/each}
            </select>
        </label>

        <div class="actions">
            <button type="button" class="cancel" onclick={() => saveModal.finish()} disabled={saving}>
                Cancel
            </button>
            <button type="button" class="ok" onclick={confirm} disabled={saving}>
                {saving ? 'Saving…' : 'Save'}
            </button>
        </div>
    </div>
</Modal>

<style>
    .save-body {
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
