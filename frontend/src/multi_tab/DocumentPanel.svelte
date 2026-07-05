<script lang="ts">
    import LeftSidebar from '../ui/LeftSidebar.svelte';
    import TabStrip from './TabStrip.svelte';
    import ToolOptionsBar from '../ui/ToolOptionsBar.svelte';
    import { canvasSlot } from './canvasSlot.svelte';

    // The canvas itself lives in the persistent `CanvasOverlay`, not here — this
    // panel only reserves the space and publishes its rect. Registering on mount
    // and clearing on destroy lets the overlay follow the panel as it's tiled,
    // and hide when this panel is an inactive tab (unmounted).
    function canvasMount(node: HTMLElement) {
        canvasSlot.set(node);
        return { destroy: () => canvasSlot.clear(node) };
    }
</script>

<!-- The tool bar is part of the canvas unit: it, the document tab strip, the
     canvas, and the tool-options bar travel together as one Document panel. -->
<div class="document-panel">
    <LeftSidebar />
    <div class="doc-main">
        <TabStrip />
        <div class="canvas-region" use:canvasMount></div>
        <ToolOptionsBar />
    </div>
</div>

<style>
    .document-panel {
        display: flex;
        flex-direction: row;
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .doc-main {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    /* Empty on purpose — reserves the canvas area; `CanvasOverlay` renders the
       actual WebGPU canvases positioned over this rect. */
    .canvas-region {
        flex: 1;
        min-width: 0;
        min-height: 0;
    }
</style>
