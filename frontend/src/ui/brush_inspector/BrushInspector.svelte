<script lang="ts">
    import { inspector } from '../../state/brush_inspector.svelte';
    import PresetSummary from './PresetSummary.svelte';
    import ParamsTable from './ParamsTable.svelte';
    import ResourceGallery from './ResourceGallery.svelte';

    let dragOver = $state(false);

    function onDrop(e: DragEvent): void {
        e.preventDefault();
        dragOver = false;
        const file = e.dataTransfer?.files[0];
        if (file) inspector.load(file);
    }

    function onDragOver(e: DragEvent): void {
        e.preventDefault();
        dragOver = true;
    }

    function onDragLeave(): void {
        dragOver = false;
    }

    function onPick(e: Event): void {
        const input = e.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        if (file) inspector.load(file);
    }
</script>

<div class="page">
    <header class="page-header">
        <h1>Krita Brush Inspector</h1>
        <p class="byline">Drop a <code>.kpp</code> preset to see every chunk, every param, every embedded resource.</p>
    </header>

    <div
        class="dropzone"
        class:dragOver
        ondrop={onDrop}
        ondragover={onDragOver}
        ondragleave={onDragLeave}
        role="button"
        tabindex="0"
    >
        {#if inspector.loading}
            <p>parsing…</p>
        {:else if inspector.file}
            <button class="reset" onclick={() => inspector.clear()}>load another</button>
        {:else}
            <p>drop a <code>.kpp</code> file here, or</p>
            <label class="pick">
                pick a file
                <input type="file" accept=".kpp" onchange={onPick} />
            </label>
        {/if}
    </div>

    {#if inspector.error}
        <div class="error">
            <strong>parse failed</strong>
            <pre>{inspector.error}</pre>
        </div>
    {/if}

    {#if inspector.file}
        <PresetSummary
            fileName={inspector.file.name}
            byteLength={inspector.file.byteLength}
            preset={inspector.file.preset}
        />
        <ResourceGallery resources={inspector.file.preset.resources} />
        <ParamsTable params={inspector.file.preset.params} />
        <details class="raw-xml">
            <summary>Raw preset XML (resource payloads elided)</summary>
            <pre>{inspector.file.preset.preset_xml_elided}</pre>
        </details>
    {/if}
</div>

<style>
    /* The main editor sets `body { overflow: hidden; user-select: none }`
       in reset.css. The inspector lives in its own fixed layer so it can
       scroll and select text without fighting that. */
    .page {
        position: fixed;
        inset: 0;
        overflow: auto;
        background: var(--bg);
        color: var(--text);
        padding: 32px;
        display: flex;
        flex-direction: column;
        gap: 16px;
        font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
        user-select: text;
        -webkit-user-select: text;
    }
    .page-header h1 {
        margin: 0 0 4px;
        font-size: 1.4rem;
    }
    .byline {
        margin: 0;
        color: var(--text-muted);
    }
    .byline code {
        background: var(--bg-hover);
        padding: 1px 5px;
        border-radius: 3px;
    }
    .dropzone {
        border: 2px dashed var(--bg-hover);
        border-radius: var(--radius-md);
        padding: 32px;
        text-align: center;
        color: var(--text-muted);
        transition: border-color var(--transition-fast), background var(--transition-fast);
    }
    .dropzone.dragOver {
        border-color: var(--accent);
        background: var(--bg-raised);
    }
    .dropzone p {
        margin: 0 0 12px;
    }
    .dropzone code {
        background: var(--bg-hover);
        padding: 1px 5px;
        border-radius: 3px;
        color: var(--text);
    }
    .pick {
        display: inline-block;
        background: var(--accent);
        color: white;
        padding: 8px 16px;
        border-radius: var(--radius-sm);
        cursor: pointer;
    }
    .pick input {
        display: none;
    }
    .reset {
        background: var(--bg-hover);
        color: var(--text);
        border: none;
        padding: 8px 16px;
        border-radius: var(--radius-sm);
        cursor: pointer;
        font-family: inherit;
    }
    .error {
        background: var(--bg-raised);
        border-left: 3px solid var(--danger);
        padding: 12px 16px;
        border-radius: var(--radius-sm);
    }
    .error strong {
        color: var(--danger);
        display: block;
        margin-bottom: 4px;
    }
    .error pre {
        margin: 0;
        font-size: 0.85rem;
        white-space: pre-wrap;
    }
    .raw-xml {
        background: var(--bg-raised);
        border-radius: var(--radius-md);
        padding: 16px;
    }
    .raw-xml summary {
        cursor: pointer;
        color: var(--text);
        font-size: 1.05rem;
        font-weight: 500;
    }
    .raw-xml pre {
        margin: 12px 0 0;
        padding: 12px;
        background: var(--bg);
        border-radius: var(--radius-sm);
        overflow-y: auto;
        white-space: pre-wrap;
        word-break: break-word;
        font-size: 0.8rem;
        max-height: 600px;
        font-family: monospace;
        line-height: 1.4;
    }
</style>
