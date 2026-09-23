<script lang="ts">
    import { brushGraph } from '../state/brush_graph.svelte';
    import BrushBuilder from './brush_builder/BrushBuilder.svelte';
    import { pointerDrag } from '../lib/pointerDrag';

    let builderHeight = $state(33); // vh units

    let startHeight = 0;

    function startResize() {
        startHeight = builderHeight;
    }

    function onResizeMove(_dx: number, dy: number) {
        const vh = window.innerHeight / 100;
        // Dragging up grows the panel.
        builderHeight = Math.min(80, Math.max(15, startHeight - dy / vh));
    }
</script>

{#if brushGraph.isOpen}
    {#if !brushGraph.fullscreen}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="resize-handle"
            use:pointerDrag={{ onStart: startResize, onMove: onResizeMove }}
        ></div>
    {/if}
    <div
        class="builder-panel"
        class:fullscreen={brushGraph.fullscreen}
        style={brushGraph.fullscreen ? '' : `height: ${builderHeight}vh`}
    >
        <BrushBuilder />
    </div>
{/if}

<style>
    .resize-handle {
        height: 5px;
        cursor: ns-resize;
        background: transparent;
        flex-shrink: 0;
        transition: background 0.1s;
    }
    .resize-handle:hover,
    .resize-handle:active {
        background: var(--accent);
    }

    .builder-panel {
        min-height: 100px;
        border-bottom: 1px solid var(--bg-hover);
    }

    /* In fullscreen the bottom-area is pinned to the window (see
     * ToolOptionsBar); the builder fills everything below the tool-options
     * strip rather than a fixed vh height. */
    .builder-panel.fullscreen {
        flex: 1;
        min-height: 0;
        border-bottom: none;
    }
</style>
