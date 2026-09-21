<script lang="ts">
    import { app } from '../state/app.svelte';
    import { toolRegistry } from '../tools/registry';
    import { brushGraph } from '../state/brush_graph.svelte';
    import FgBgSwatches from './color/FgBgSwatches.svelte';

    // The strip itself is always mounted: only the content inside (and
    // any optional panel above) varies per tool. Keeping the same DOM
    // node across tool switches avoids a flicker / layout reflow.
    let tool = $derived(toolRegistry.get(app.activeToolId));
    let Options = $derived(tool?.optionsComponent);
    let Panel = $derived(tool?.panelComponent);
</script>

<div class="bottom-area" class:fullscreen={brushGraph.fullscreen}>
    <div class="tool-options">
        <!-- Global color chrome, not a per-tool option. It sits inside
             `.tool-options` rather than beside this bar because `.bottom-area`
             also hosts a tool panel (the brush builder) and goes fixed and
             fullscreen with it; a sibling would flank that panel and then
             disappear. A consequence of living here: the swatches stay
             reachable inside the fullscreen brush builder, where the old
             vertical toolbar was covered over. -->
        <div class="color-zone">
            <FgBgSwatches mode="popup" />
        </div>
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
        /* Minimum (not fixed) height: the bar is 40px at rest, sized to
         * fit the tallest control (~32px) with a 4px breather, but grows
         * taller when controls wrap onto extra lines in a narrow window
         * (see ToolBarLayout `.center`). */
        min-height: 40px;
    }

    /* Separated from the tool's own controls the same way the tool strip
       separates its groups. */
    .color-zone {
        display: flex;
        align-items: center;
        flex: none;
        padding-right: 8px;
        margin-right: 4px;
        border-right: 1px solid var(--bg-hover);
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
