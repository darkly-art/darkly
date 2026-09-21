<script lang="ts">
    import ToolStrip from '../ui/ToolStrip.svelte';
    import HamburgerMenu from '../ui/HamburgerMenu.svelte';
    import TabStrip from './TabStrip.svelte';
    import ToolOptionsBar from '../ui/ToolOptionsBar.svelte';
    import { canvasSlot } from './canvasSlot.svelte';
    import { menuBar } from '../state/menuBar.svelte';

    // The canvas itself lives in the persistent `CanvasOverlay`, not here; this
    // panel only reserves the space and publishes its rect. Registering on mount
    // and clearing on destroy lets the overlay follow the panel as it's tiled,
    // and hide when this panel is an inactive tab (unmounted).
    function canvasMount(node: HTMLElement) {
        canvasSlot.set(node);
        return { destroy: () => canvasSlot.clear(node) };
    }
</script>

<!-- One column, full width at every row: the document tab strip, the canvas,
     and the tool-options bar. The tool strip is not a fourth row but an overlay
     floating on the canvas region, so the canvas reaches the panel's left edge.
     The hamburger and the foreground/background swatches ride in the top and
     bottom rows because a popup raised inside the tool strip would be trapped
     under its stacking context (see ToolStrip). -->
<div class="document-panel">
    <div class="doc-top">
        {#if !menuBar.pinned}
            <HamburgerMenu />
        {/if}
        <TabStrip />
    </div>
    <div class="canvas-region" use:canvasMount>
        <ToolStrip />
    </div>
    <ToolOptionsBar />
</div>

<style>
    .document-panel {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    /* Carries the top bar's fill and underline so both run the full panel
       width; the tab strip inside is transparent and stops where the tabs do. */
    .doc-top {
        display: flex;
        align-items: stretch;
        flex: 0 0 auto;
        min-width: 0;
        background: var(--bg-raised);
        border-bottom: 1px solid var(--bg-hover);
    }

    /* Holds no canvas of its own: it reserves the area and is the containing
       block for the floating tool strip. `CanvasOverlay` renders the actual
       WebGPU canvases positioned over this rect, and an absolutely positioned
       child does not affect that rect, so `canvasSlot` and the overlay's
       ResizeObserver are unaffected. Must never gain a `z-index` or
       `isolation`: either would make it a stacking context and bury the tool
       strip behind the overlay's canvases. */
    .canvas-region {
        position: relative;
        flex: 1;
        min-width: 0;
        min-height: 0;
    }
</style>
