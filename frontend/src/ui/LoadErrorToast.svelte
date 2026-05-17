<script lang="ts">
    import { loadError } from '../state/loadError.svelte';

    // Format the payload into a title + body. The body lists every
    // missing `<registry>/<type_id>` for the most common case
    // (`unsupportedFeatures`); other kinds fall back to the verbatim
    // message the engine produced.
    const view = $derived.by(() => {
        const p = loadError.payload;
        if (!p) return null;
        switch (p.kind) {
            case 'unsupportedFeatures':
                return {
                    title: "This file needs features your Darkly doesn't support",
                    body: ('missing' in p ? p.missing : [])
                        .map(m => `• ${m}`)
                        .join('\n'),
                    cta: 'Update Darkly to open it.',
                };
            case 'containerTooNew':
                return {
                    title: 'This file was made by a newer Darkly',
                    body: 'found' in p && 'supported' in p
                        ? `container_version: found ${p.found}, supports ${p.supported}`
                        : '',
                    cta: 'Update Darkly to open it.',
                };
            case 'corruptManifest':
            case 'unknownTypeId':
                return {
                    title: 'This file is malformed',
                    body: p.message ?? '',
                    cta: '',
                };
            default:
                return {
                    title: 'Failed to open file',
                    body: p.message ?? '',
                    cta: '',
                };
        }
    });
</script>

{#if view}
    <div class="load-error" role="alert">
        <div class="header">
            <strong class="title">{view.title}</strong>
            <button
                type="button"
                class="close"
                aria-label="Dismiss"
                onclick={() => loadError.dismiss()}
            >×</button>
        </div>
        {#if view.body}
            <pre class="body">{view.body}</pre>
        {/if}
        {#if view.cta}
            <p class="cta">{view.cta}</p>
        {/if}
    </div>
{/if}

<style>
    .load-error {
        position: fixed;
        bottom: 24px;
        right: 24px;
        z-index: 1100;
        background: var(--bg-active);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-left: 4px solid #f44336;
        border-radius: 6px;
        padding: 12px 16px 14px;
        font-size: 13px;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.55);
        max-width: 420px;
        min-width: 280px;
    }

    .header {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 12px;
    }

    .title {
        font-size: 13px;
        font-weight: 600;
        line-height: 1.3;
    }

    .close {
        background: transparent;
        border: none;
        color: var(--text-muted);
        font-size: 20px;
        line-height: 1;
        cursor: pointer;
        padding: 0 4px;
        margin-top: -2px;
        border-radius: 4px;
    }

    .close:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .body {
        margin: 8px 0 0;
        padding: 0;
        font-family: var(--font-mono, monospace);
        font-size: 12px;
        color: var(--text-muted);
        white-space: pre-wrap;
        word-break: break-word;
    }

    .cta {
        margin: 8px 0 0;
        font-size: 12px;
        color: var(--text);
    }
</style>
