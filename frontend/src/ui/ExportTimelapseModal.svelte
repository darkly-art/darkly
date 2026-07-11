<script lang="ts">
    import Modal from './Modal.svelte';
    import { exportTimelapse } from '../state/exportTimelapse.svelte';
    import { app, getActiveInstance } from '../state/app.svelte';
    import { downloadBlob, sanitizeFilename } from '../storage';
    import { processRecording } from '../recording/recorder.svelte';
    import {
        exportTimelapseGif,
        exportTimelapseMp4,
        readRecordingInfo,
        type RecordingInfo,
    } from '../recording/exportTimelapse';

    type Format = 'mp4' | 'gif';

    const FORMATS: Array<{ id: Format; label: string; ext: string }> = [
        { id: 'mp4', label: 'MP4', ext: 'mp4' },
        { id: 'gif', label: 'GIF', ext: 'gif' },
    ];

    const FPS_CHOICES = [10, 15, 24, 30, 60];

    let format = $state<Format>('mp4');
    let fps = $state(30);
    let baseName = $state('darkly-timelapse');
    let exporting = $state(false);
    let info = $state<RecordingInfo | null>(null);
    let loading = $state(false);

    // Re-read the recording summary each time the modal opens — the
    // recording grows while the user paints.
    $effect(() => {
        if (exportTimelapse.open) void refreshInfo();
    });

    async function refreshInfo() {
        const inst = getActiveInstance();
        if (!inst) return;
        loading = true;
        try {
            info = await readRecordingInfo(inst);
        } catch (e) {
            console.error('[export-timelapse] failed to read recording', e);
            info = null;
        } finally {
            loading = false;
        }
    }

    function close() {
        if (exporting) return;
        exportTimelapse.open = false;
    }

    function formatBytes(n: number): string {
        if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
        if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
        return `${Math.max(1, Math.round(n / 1e3))} kB`;
    }

    function formatDuration(secs: number): string {
        const m = Math.floor(secs / 60);
        const s = Math.round(secs % 60);
        return m > 0 ? `${m}m ${s}s` : `${s}s`;
    }

    async function confirm() {
        const inst = getActiveInstance();
        if (!inst || exporting || !info) return;
        exporting = true;
        try {
            const blob =
                format === 'mp4'
                    ? await exportTimelapseMp4(inst, fps)
                    : await exportTimelapseGif(inst, fps);
            const ext = FORMATS.find((f) => f.id === format)!.ext;
            const filename = `${sanitizeFilename(baseName) || 'darkly-timelapse'}.${ext}`;
            downloadBlob(blob, filename);
            exportTimelapse.open = false;
        } catch (e) {
            console.error('[export-timelapse] export failed', e);
            alert('Timelapse export failed — see console for details.');
        } finally {
            exporting = false;
        }
    }

    async function deleteRecording() {
        const inst = getActiveInstance();
        if (!inst || exporting) return;
        try {
            await processRecording.deleteRecording(inst);
        } finally {
            await refreshInfo();
        }
    }
</script>

<Modal bind:open={exportTimelapse.open} title="Export Timelapse" size="sm">
    <div class="export-body">
        {#if loading}
            <p class="info">Reading recording…</p>
        {:else if !info}
            <p class="info">
                No process recording yet — paint with recording enabled
                (Settings → Recording) and come back.
            </p>
        {:else}
            <p class="info">
                {info.frameCount} frames · {formatDuration(info.frameCount / fps)} at
                {fps} fps · {info.width}×{info.height} · {formatBytes(info.byteSize)} recorded
            </p>

            <label class="row">
                <span class="label">Filename</span>
                <div class="filename">
                    <input
                        type="text"
                        bind:value={baseName}
                        placeholder="darkly-timelapse"
                        disabled={exporting}
                    />
                    <span class="ext">.{FORMATS.find((f) => f.id === format)!.ext}</span>
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

            <label class="row">
                <span class="label">Playback speed (fps)</span>
                <select bind:value={fps} disabled={exporting}>
                    {#each FPS_CHOICES as v (v)}
                        <option value={v}>{v}</option>
                    {/each}
                </select>
            </label>
        {/if}

        <div class="actions">
            {#if info}
                <button
                    type="button"
                    class="danger"
                    onclick={deleteRecording}
                    disabled={exporting}
                >
                    Delete recording
                </button>
            {/if}
            <span class="spacer"></span>
            <button type="button" class="cancel" onclick={close} disabled={exporting}>
                Cancel
            </button>
            <button
                type="button"
                class="ok"
                onclick={confirm}
                disabled={exporting || !info}
            >
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
        min-width: 340px;
    }

    .info {
        margin: 0;
        font-size: 12px;
        color: var(--text-muted);
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

    .actions button:disabled {
        opacity: 0.5;
        cursor: default;
    }

    .actions .danger {
        color: var(--error, #e5484d);
        border-color: var(--bg-hover);
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
