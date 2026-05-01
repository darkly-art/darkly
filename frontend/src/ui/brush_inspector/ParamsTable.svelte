<script lang="ts">
    import { onDestroy } from 'svelte';
    import { inspector, type KritaParam } from '../../state/brush_inspector.svelte';
    import SensorCurveSparkline from './SensorCurveSparkline.svelte';
    import XmlNodeView from './XmlNodeView.svelte';

    interface Props {
        params: KritaParam[];
    }
    let { params }: Props = $props();

    let filter = $state('');

    // Group by the prefix before the first '/' — Krita uses
    // "MaskingBrush/Preset/FlowSensor"-style namespaced names.
    interface GroupItem {
        param: KritaParam;
        index: number; // index into the original params[] (for WASM lookups)
    }
    interface Group {
        name: string;
        items: GroupItem[];
    }
    const groups = $derived.by(() => {
        const filterLower = filter.trim().toLowerCase();
        const map = new Map<string, GroupItem[]>();
        for (let i = 0; i < params.length; i++) {
            const p = params[i];
            if (filterLower && !matches(p, filterLower)) continue;
            const slash = p.name.indexOf('/');
            const key = slash > 0 ? p.name.slice(0, slash) : '(root)';
            if (!map.has(key)) map.set(key, []);
            map.get(key)!.push({ param: p, index: i });
        }
        return [...map.entries()]
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([name, items]) => ({ name, items }) as Group);
    });

    function matches(p: KritaParam, q: string): boolean {
        if (p.name.toLowerCase().includes(q)) return true;
        if (p.raw_value.toLowerCase().includes(q)) return true;
        return false;
    }

    // Lazily mint blob URLs for embedded-image params; revoke on unmount.
    const imageUrls = new Map<number, string>();

    function imageUrlFor(paramIndex: number): string | null {
        if (imageUrls.has(paramIndex)) return imageUrls.get(paramIndex)!;
        const url = inspector.paramImageBlobUrl(paramIndex);
        if (url) imageUrls.set(paramIndex, url);
        return url;
    }

    function formatBytes(n: number): string {
        if (n < 1024) return `${n} B`;
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
        return `${(n / 1024 / 1024).toFixed(2)} MB`;
    }

    onDestroy(() => {
        for (const url of imageUrls.values()) URL.revokeObjectURL(url);
        imageUrls.clear();
    });
</script>

<section class="params">
    <header>
        <h3>Params ({params.length})</h3>
        <input
            type="search"
            placeholder="filter by name or value..."
            bind:value={filter}
        />
    </header>

    {#if groups.length === 0}
        <p class="empty">No params match the current filter.</p>
    {/if}

    {#each groups as group (group.name)}
        <details open>
            <summary>
                <span class="group-name">{group.name}</span>
                <span class="group-count">{group.items.length}</span>
            </summary>
            <table>
                <colgroup>
                    <col class="col-name" />
                    <col class="col-value" />
                </colgroup>
                <tbody>
                    {#each group.items as item (item.param.name)}
                        {@const param = item.param}
                        {@const decoded = param.decoded}
                        <tr>
                            <td class="name"><code>{param.name}</code></td>
                            <td class="value">
                                {#if decoded.kind === 'plain'}
                                    <code>{decoded.value || '(empty)'}</code>
                                {:else if decoded.kind === 'curve'}
                                    <div class="curve-cell">
                                        <SensorCurveSparkline points={decoded.points} />
                                        <span class="curve-points">
                                            {decoded.points.length} pts:
                                            {decoded.points
                                                .map(
                                                    ([x, y]: [number, number]) =>
                                                        `(${x.toFixed(2)},${y.toFixed(2)})`,
                                                )
                                                .join(' ')}
                                        </span>
                                    </div>
                                {:else if decoded.kind === 'sensor_xml'}
                                    <div>
                                        <span class="sensor-id">
                                            sensor:
                                            <code>{decoded.sensor_id ?? '?'}</code>
                                        </span>
                                        <details>
                                            <summary>raw xml</summary>
                                            <pre>{decoded.xml}</pre>
                                        </details>
                                    </div>
                                {:else if decoded.kind === 'nested_xml'}
                                    <XmlNodeView node={decoded.root} />
                                {:else if decoded.kind === 'bytearray'}
                                    <span class="bytes">
                                        bytearray, {decoded.byte_length} bytes (base64)
                                    </span>
                                {:else if decoded.kind === 'embedded_image'}
                                    {@const url = imageUrlFor(item.index)}
                                    <div class="image-cell">
                                        {#if url}
                                            <img src={url} alt={param.name} />
                                        {/if}
                                        <span class="image-meta">
                                            {decoded.format.kind}{#if decoded.format.kind === 'png' && decoded.format.width !== null}
                                                &nbsp;{decoded.format.width}×{decoded.format.height}{/if},
                                            {formatBytes(decoded.byte_length)}
                                        </span>
                                    </div>
                                {/if}
                            </td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        </details>
    {/each}
</section>

<style>
    .params {
        background: var(--bg-raised);
        border-radius: var(--radius-md);
        padding: 16px;
    }
    header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        margin-bottom: 12px;
    }
    h3 {
        margin: 0;
        color: var(--text);
        font-size: 1.05rem;
    }
    input[type='search'] {
        flex: 1;
        max-width: 320px;
        padding: 6px 10px;
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        font-family: inherit;
        font-size: 0.85rem;
    }
    details {
        margin-bottom: 8px;
    }
    summary {
        cursor: pointer;
        color: var(--text);
        padding: 4px 0;
    }
    .group-name {
        font-weight: 500;
    }
    .group-count {
        color: var(--text-muted);
        margin-left: 8px;
        font-size: 0.85rem;
    }
    table {
        width: 100%;
        border-collapse: collapse;
        font-size: 0.85rem;
        table-layout: fixed;
    }
    .col-name {
        width: 30%;
    }
    .col-value {
        width: auto;
    }
    td {
        padding: 4px 8px;
        vertical-align: top;
        border-bottom: 1px solid var(--bg-hover);
        word-break: break-word;
    }
    td.name code {
        background: transparent;
        padding: 0;
        color: var(--text);
    }
    td.value code {
        background: var(--bg-hover);
        padding: 1px 5px;
        border-radius: 3px;
    }
    .curve-cell {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .curve-points {
        font-family: monospace;
        color: var(--text-muted);
        font-size: 0.8rem;
    }
    .sensor-id code {
        background: var(--bg-hover);
        padding: 1px 5px;
        border-radius: 3px;
    }
    .bytes {
        color: var(--text-muted);
        font-style: italic;
    }
    .image-cell {
        display: flex;
        align-items: flex-start;
        gap: 12px;
    }
    .image-cell img {
        max-width: 128px;
        max-height: 128px;
        background: var(--canvas-bg);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        image-rendering: pixelated;
    }
    .image-meta {
        color: var(--text-muted);
        font-size: 0.8rem;
        align-self: center;
    }
    pre {
        background: var(--bg);
        color: var(--text);
        padding: 8px;
        border-radius: var(--radius-sm);
        overflow-x: auto;
        font-size: 0.8rem;
        margin: 4px 0 0;
    }
    .empty {
        color: var(--text-muted);
        font-style: italic;
    }
</style>
