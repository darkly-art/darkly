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
            <span class="tool-name">{tool ? app.displayName('tools', tool.id) : ''}</span>
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
        background: var(--canvas-bg);
        flex-shrink: 0;
        /* Minimum (not fixed) height: the bar is 40px at rest — sized to
         * fit the tallest control (~32px) with a 4px breather — but grows
         * taller when controls wrap onto extra lines in a narrow window
         * (see ToolBarLayout `.center`). */
        min-height: 40px;
    }

    .tool-name {
        display: flex;
        align-items: center;
        font-size: 11px;
        font-weight: 600;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        padding: 0 12px;
    }

    .spacer {
        flex: 1;
    }
</style>
