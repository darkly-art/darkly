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
        <label class="field">
            <span class="field-label">Filename</span>
            <div class="field-num filename">
                <input
                    type="text"
                    bind:value={baseName}
                    placeholder="darkly-document"
                    disabled={saving}
                />
                <span class="ext">.{SAVE_FORMATS[format].ext}</span>
            </div>
        </label>

        <label class="field">
            <span class="field-label">Type</span>
            <select bind:value={format} disabled={saving}>
                {#each SAVE_FORMAT_ORDER as f (f)}
                    <option value={f}>{LABELS[f]}</option>
                {/each}
            </select>
        </label>

        <div class="dialog-actions">
            <button type="button" class="btn" onclick={() => saveModal.finish()} disabled={saving}>
                Cancel
            </button>
            <button type="button" class="btn primary" onclick={confirm} disabled={saving}>
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

    /* The extension rides inside the field frame, so the input gives up its
     * own padding to `.field-num`. */
    .filename .ext {
        color: var(--text-muted);
        font-family: var(--font-mono, monospace);
        font-size: 12px;
    }

    select {
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        padding: 6px 8px;
        font: inherit;
    }
</style>
