<script lang="ts">
    import type { AdapterInfo, GpuCheckFailure } from '../gpu';
    import type { Platform } from '../platform';
    import {
        allInstructions,
        instructionsFor,
        type FlagLink,
        type Instructions,
    } from '../gpuErrorInstructions';

    let { failure, platform }: { failure: GpuCheckFailure; platform: Platform } =
        $props();

    const primary = $derived(instructionsFor(platform));
    const others = $derived(
        allInstructions().filter(({ instructions }) => instructions !== primary),
    );

    const headline = 'Darkly needs GPU.';

    function causeFor(f: GpuCheckFailure): string {
        switch (f.reason) {
            case 'no-webgpu':
                return 'Your browser does not expose the WebGPU API.';
            case 'no-adapter':
                return 'Your browser could not find a GPU adapter.';
            case 'fallback-adapter':
                return 'Your browser fell back to a software renderer.';
        }
    }

    const adapterInfo: AdapterInfo | undefined = $derived(
        failure.reason === 'fallback-adapter' ? failure.adapterInfo : undefined,
    );

    let copied = $state<string | null>(null);
    let copyTimer: ReturnType<typeof setTimeout> | null = null;

    async function copyFlag(url: string) {
        try {
            await navigator.clipboard.writeText(url);
            copied = url;
            if (copyTimer) clearTimeout(copyTimer);
            copyTimer = setTimeout(() => {
                copied = null;
            }, 1500);
        } catch {
            // Clipboard unavailable — fall through silently. User can still
            // read and type the URL manually.
        }
    }
</script>

<div class="gpu-error-root">
    <main class="gpu-error">
        <h1>{headline}</h1>
        <p class="cause">{causeFor(failure)}</p>
        <p class="lede">
            Darkly is a GPU-accelerated painting app and cannot run on a software renderer.
            Follow the steps below to enable hardware acceleration, then reload this page.
        </p>

        <section class="primary">
            <h2>{primary.title}</h2>
            {@render instructionBlock(primary)}
        </section>

        {#if adapterInfo}
            <details class="adapter-info">
                <summary>What your browser reported</summary>
                <dl>
                    <dt>vendor</dt>
                    <dd>{adapterInfo.vendor || '(empty)'}</dd>
                    <dt>architecture</dt>
                    <dd>{adapterInfo.architecture || '(empty)'}</dd>
                    <dt>device</dt>
                    <dd>{adapterInfo.device || '(empty)'}</dd>
                    <dt>description</dt>
                    <dd>{adapterInfo.description || '(empty)'}</dd>
                </dl>
                <p class="hint">
                    Include these values if you file a bug — they describe the adapter the browser handed us.
                </p>
            </details>
        {/if}

        {#if others.length > 0}
            <details class="other-platforms">
                <summary>Instructions for other platforms</summary>
                {#each others as { key, instructions } (key)}
                    <section>
                        <h3>{instructions.title}</h3>
                        {@render instructionBlock(instructions)}
                    </section>
                {/each}
            </details>
        {/if}
    </main>
</div>

{#snippet instructionBlock(instr: Instructions)}
    <ol>
        {#each instr.steps as step}
            <li>{step}</li>
        {/each}
    </ol>

    {#if instr.flags.length > 0}
        <ul class="flag-list">
            {#each instr.flags as flag (flag.url)}
                {@render flagRow(flag)}
            {/each}
        </ul>
    {/if}

    {#if instr.diagnosticUrl}
        <p class="diagnostic">
            To check what your browser reports, open
            <code>{instr.diagnosticUrl}</code>
            <button
                type="button"
                class="copy-btn"
                onclick={() => copyFlag(instr.diagnosticUrl!)}
            >
                {copied === instr.diagnosticUrl ? 'Copied' : 'Copy'}
            </button>
        </p>
    {/if}

    {#if instr.note}
        <p class="note">{instr.note}</p>
    {/if}
{/snippet}

{#snippet flagRow(flag: FlagLink)}
    <li>
        <code>{flag.url}</code>
        <button
            type="button"
            class="copy-btn"
            onclick={() => copyFlag(flag.url)}
        >
            {copied === flag.url ? 'Copied' : 'Copy'}
        </button>
        <span class="flag-action">{flag.action}</span>
    </li>
{/snippet}

<style>
    .gpu-error-root {
        position: fixed;
        inset: 0;
        overflow: auto;
        background: var(--bg);
        color: var(--text);
        font-family: system-ui, -apple-system, sans-serif;
    }

    .gpu-error {
        max-width: 640px;
        margin: 0 auto;
        padding: 48px 24px 96px;
    }

    h1 {
        font-size: 24px;
        margin: 0 0 12px;
        color: var(--text);
    }

    h2 {
        font-size: 18px;
        margin: 32px 0 12px;
        color: var(--text);
    }

    h3 {
        font-size: 15px;
        margin: 16px 0 8px;
        color: var(--text);
    }

    .cause {
        font-size: 15px;
        color: var(--danger);
        margin: 0 0 16px;
    }

    .lede {
        font-size: 14px;
        line-height: 1.5;
        color: var(--text-muted);
        margin: 0 0 24px;
    }

    ol, ul {
        padding-left: 24px;
        margin: 8px 0;
        line-height: 1.6;
    }

    ol li, ul li {
        font-size: 14px;
        margin: 4px 0;
    }

    .flag-list {
        list-style: none;
        padding-left: 0;
        display: flex;
        flex-direction: column;
        gap: 6px;
        margin: 12px 0;
    }

    .flag-list li {
        display: flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
    }

    code {
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        background: var(--bg-hover);
        color: var(--text);
        padding: 3px 8px;
        border-radius: var(--radius-sm);
        font-size: 13px;
        user-select: text;
    }

    .copy-btn {
        font-family: inherit;
        font-size: 12px;
        background: var(--bg-active);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        padding: 4px 10px;
        cursor: pointer;
        transition: background var(--transition-fast);
    }

    .copy-btn:hover {
        background: var(--bg-hover);
    }

    .flag-action {
        font-size: 13px;
        color: var(--text-muted);
    }

    .diagnostic {
        font-size: 13px;
        color: var(--text-muted);
        margin: 12px 0;
        display: flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
    }

    .note {
        font-size: 13px;
        color: var(--text-muted);
        margin-top: 12px;
        padding: 10px 12px;
        background: var(--bg-hover);
        border-left: 3px solid var(--accent);
        border-radius: var(--radius-sm);
    }

    details {
        margin-top: 32px;
        border-top: 1px solid var(--bg-hover);
        padding-top: 16px;
    }

    summary {
        cursor: pointer;
        font-size: 14px;
        color: var(--text);
        padding: 8px 0;
        user-select: none;
    }

    summary:hover {
        color: var(--accent);
    }

    dl {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 4px 16px;
        margin: 12px 0;
        font-size: 13px;
    }

    dt {
        color: var(--text-muted);
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }

    dd {
        margin: 0;
        color: var(--text);
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        word-break: break-word;
    }

    .hint {
        font-size: 12px;
        color: var(--text-muted);
        margin: 8px 0 0;
    }

    .adapter-info p.hint,
    .other-platforms {
        margin-top: 16px;
    }

    .other-platforms section {
        margin-top: 20px;
    }
</style>
