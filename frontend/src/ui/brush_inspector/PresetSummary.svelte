<script lang="ts">
    import type { KritaPreset } from '../../state/brush_inspector.svelte';

    interface Props {
        fileName: string;
        byteLength: number;
        preset: KritaPreset;
    }
    let { fileName, byteLength, preset }: Props = $props();

    function formatBytes(n: number): string {
        if (n < 1024) return `${n} B`;
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
        return `${(n / 1024 / 1024).toFixed(2)} MB`;
    }
</script>

<section class="summary">
    <header>
        <h2>{preset.preset_name ?? fileName}</h2>
        <span class="filename">{fileName}</span>
    </header>
    <dl>
        <dt>paintop</dt>
        <dd>
            <code>{preset.paintop_id}</code>
            {#if preset.paintop_description}
                <span class="hint">{preset.paintop_description}</span>
            {/if}
        </dd>
        <dt>format version</dt>
        <dd><code>{preset.format_version}</code></dd>
        <dt>file size</dt>
        <dd>{formatBytes(byteLength)}</dd>
        <dt>thumbnail</dt>
        <dd>
            {preset.png.width}×{preset.png.height} {preset.png.color_type} ({preset.png.bit_depth}-bit)
        </dd>
        <dt>params</dt>
        <dd>{preset.params.length}</dd>
        <dt>embedded resources</dt>
        <dd>
            {preset.resources.length}
            {#if preset.embedded_resources_attr !== null && preset.embedded_resources_attr !== preset.resources.length}
                <span class="warn">(XML attr says {preset.embedded_resources_attr})</span>
            {/if}
        </dd>
    </dl>

    <details class="chunks">
        <summary>PNG chunks ({preset.png.chunks.length})</summary>
        <table>
            <thead>
                <tr>
                    <th>type</th>
                    <th>bytes</th>
                    <th>keyword</th>
                    <th>decoded</th>
                </tr>
            </thead>
            <tbody>
                {#each preset.png.chunks as chunk (chunk.chunk_type + chunk.byte_length)}
                    <tr>
                        <td><code>{chunk.chunk_type}</code></td>
                        <td class="num">{chunk.byte_length}</td>
                        <td>{chunk.text_keyword ?? ''}</td>
                        <td class="num">
                            {chunk.text_length !== null ? `${chunk.text_length} chars` : ''}
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
    </details>
</section>

<style>
    .summary {
        background: var(--bg-raised);
        border-radius: var(--radius-md);
        padding: 16px;
    }
    header {
        display: flex;
        align-items: baseline;
        gap: 12px;
        margin-bottom: 12px;
    }
    h2 {
        margin: 0;
        font-size: 1.2rem;
        color: var(--text);
    }
    .filename {
        color: var(--text-muted);
        font-family: monospace;
        font-size: 0.85rem;
    }
    dl {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 4px 16px;
        margin: 0 0 12px;
    }
    dt {
        color: var(--text-muted);
        font-size: 0.85rem;
    }
    dd {
        margin: 0;
        color: var(--text);
    }
    code {
        background: var(--bg-hover);
        padding: 1px 5px;
        border-radius: 3px;
        font-size: 0.85rem;
    }
    .hint {
        color: var(--text-muted);
        margin-left: 8px;
        font-size: 0.85rem;
    }
    .warn {
        color: var(--danger);
        margin-left: 8px;
        font-size: 0.85rem;
    }
    .chunks summary {
        cursor: pointer;
        color: var(--text-muted);
        font-size: 0.9rem;
    }
    .chunks table {
        width: 100%;
        margin-top: 8px;
        border-collapse: collapse;
        font-size: 0.85rem;
    }
    .chunks th,
    .chunks td {
        padding: 4px 8px;
        text-align: left;
        border-bottom: 1px solid var(--bg-hover);
    }
    .chunks th {
        color: var(--text-muted);
        font-weight: normal;
    }
    .chunks .num {
        text-align: right;
        font-variant-numeric: tabular-nums;
        color: var(--text-muted);
    }
</style>
