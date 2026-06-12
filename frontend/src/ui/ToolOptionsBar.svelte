<script lang="ts">
    import { app } from '../state/app.svelte';
    import { toolRegistry } from '../tools/registry';
    import { brushGraph } from '../state/brush_graph.svelte';

    // The strip itself is always mounted — only the content inside (and
    // any optional panel above) varies per tool. Keeping the same DOM
    // node across tool switches avoids a flicker / layout reflow.
    let tool = $derived(toolRegistry.get(app.activeToolId));
    let Options = $derived(tool?.optionsComponent);
    let Panel = $derived(tool?.panelComponent);
</script>

<div class="bottom-area" class:fullscreen={brushGraph.fullscreen}>
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

    /* Fullscreen brush builder: pin the whole bottom area to the window so
     * the tool-options strip stays at the top and the builder fills the
     * space below it. The builder panel switches to flex:1 in this mode
     * (see BrushBuilderPanel). */
    .bottom-area.fullscreen {
        position: fixed;
        inset: 0;
        z-index: 9999;
        background: var(--bg);
    }

    .tool-options {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 4px;
        padding: 4px 8px;
        background: var(--bg);
        flex-shrink: 0;
        /* Fixed height (not min-height) so the bar stays the same size
         * across tool switches and the canvas above doesn't shift when
         * the user swaps tools. Sized to fit the tallest tool's content
         * — currently the brush picker (preview strip + name + chevron),
         * which is ~32px tall; 40px total leaves a 4px breather around
         * it. Shorter content (scrubs, dropdowns) centers within the
         * extra space. `box-sizing: border-box` is set globally, so this
         * height is inclusive of padding. */
        height: 40px;
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
