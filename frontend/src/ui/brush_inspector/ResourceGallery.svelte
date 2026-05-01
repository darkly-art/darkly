<script lang="ts">
    import { onDestroy } from 'svelte';
    import { inspector, type KritaResource } from '../../state/brush_inspector.svelte';

    interface Props {
        resources: KritaResource[];
    }
    let { resources }: Props = $props();

    // Track blob URLs created so we can revoke them on unmount.
    const urls = new Map<number, string>();

    function urlFor(idx: number): string | null {
        if (urls.has(idx)) return urls.get(idx)!;
        const url = inspector.resourceBlobUrl(idx);
        if (url) urls.set(idx, url);
        return url;
    }

    function isImageish(kind: string): boolean {
        return kind === 'png' || kind === 'jpeg' || kind === 'svg';
    }

    onDestroy(() => {
        for (const url of urls.values()) URL.revokeObjectURL(url);
        urls.clear();
    });

    function formatBytes(n: number): string {
        if (n < 1024) return `${n} B`;
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
        return `${(n / 1024 / 1024).toFixed(2)} MB`;
    }
</script>

<section class="gallery">
    <h3>Embedded resources ({resources.length})</h3>
    {#if resources.length === 0}
        <p class="empty">No embedded resources. (5.0 presets only — 2.2 stores resources externally.)</p>
    {/if}
    <div class="grid">
        {#each resources as resource, idx (resource.md5sum + idx)}
            <article class="card">
                <div class="thumb">
                    {#if isImageish(resource.format.kind)}
                        {@const url = urlFor(idx)}
                        {#if url}
                            <img src={url} alt={resource.name} />
                        {/if}
                    {:else if resource.format.kind === 'unknown'}
                        <div class="fallback">
                            <div class="fallback-label">unknown format</div>
                            <code>{resource.format.magic_hex}</code>
                        </div>
                    {:else}
                        <div class="fallback">
                            <div class="fallback-label">{resource.format.kind.toUpperCase()}</div>
                            <p class="fallback-note">no native preview yet</p>
                        </div>
                    {/if}
                </div>
                <dl>
                    <dt>name</dt>
                    <dd><code>{resource.name}</code></dd>
                    <dt>filename</dt>
                    <dd><code>{resource.filename}</code></dd>
                    <dt>type</dt>
                    <dd>{resource.resource_type}</dd>
                    <dt>format</dt>
                    <dd>
                        {resource.format.kind}
                        {#if resource.format.kind === 'png' && resource.format.width !== null}
                            ({resource.format.width}×{resource.format.height})
                        {/if}
                    </dd>
                    <dt>size</dt>
                    <dd>{formatBytes(resource.byte_length)}</dd>
                    <dt>md5</dt>
                    <dd><code class="md5">{resource.md5sum}</code></dd>
                </dl>
            </article>
        {/each}
    </div>
</section>

<style>
    .gallery {
        background: var(--bg-raised);
        border-radius: var(--radius-md);
        padding: 16px;
    }
    h3 {
        margin: 0 0 12px;
        color: var(--text);
        font-size: 1.05rem;
    }
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
        gap: 12px;
    }
    .card {
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-md);
        padding: 8px;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .thumb {
        aspect-ratio: 1;
        background: var(--canvas-bg);
        border-radius: var(--radius-sm);
        display: flex;
        align-items: center;
        justify-content: center;
        overflow: hidden;
    }
    .thumb img {
        max-width: 100%;
        max-height: 100%;
        image-rendering: pixelated;
    }
    .fallback {
        text-align: center;
        padding: 12px;
    }
    .fallback-label {
        color: var(--text-muted);
        text-transform: uppercase;
        font-size: 0.8rem;
        letter-spacing: 0.05em;
        margin-bottom: 4px;
    }
    .fallback-note {
        color: var(--text-dim);
        font-size: 0.75rem;
        margin: 4px 0 0;
    }
    .fallback code {
        font-size: 0.75rem;
        color: var(--text);
        background: var(--bg-hover);
        padding: 2px 6px;
        border-radius: 3px;
    }
    dl {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 2px 8px;
        margin: 0;
        font-size: 0.8rem;
    }
    dt {
        color: var(--text-muted);
    }
    dd {
        margin: 0;
        color: var(--text);
        word-break: break-word;
    }
    code {
        font-family: monospace;
        font-size: 0.75rem;
    }
    .md5 {
        color: var(--text-muted);
    }
    .empty {
        color: var(--text-muted);
        font-style: italic;
        margin: 0;
    }
</style>
