<script lang="ts">
    import { app } from '../state/app.svelte';
    import { toolRegistry } from '../tools/registry';

    // The strip itself is always mounted — only the content inside (and
    // any optional panel above) varies per tool. Keeping the same DOM
    // node across tool switches avoids a flicker / layout reflow.
    let tool = $derived(toolRegistry.get(app.activeToolId));
    let Options = $derived(tool?.optionsComponent);
    let Panel = $derived(tool?.panelComponent);
</script>

<div class="bottom-area">
    <div class="tool-options">
        {#if Options}
            <Options />
        {:else}
            <span class="tool-name">{tool ? app.toolDisplayName(tool.id) : ''}</span>
            <div class="spacer"></div>
        {/if}
    </div>
    {#if Panel}
        <Panel />
    {/if}
</div>

<style>
    .bottom-area {
        display: flex;
        flex-direction: column;
        flex-shrink: 0;
    }

    .tool-options {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 4px;
        padding: 4px 8px;
        background: var(--bg);
        flex-shrink: 0;
        min-height: 36px;
    }

    .tool-name {
        font-size: 11px;
        font-weight: 600;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        padding: 0 4px;
    }

    .spacer {
        flex: 1;
    }
</style>
