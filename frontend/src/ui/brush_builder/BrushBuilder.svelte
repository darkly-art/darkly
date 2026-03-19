<script lang="ts">
    import { brushGraph } from '../../state/brush_graph.svelte';
    import NodeCanvas from './NodeCanvas.svelte';
    import NodePalette from './NodePalette.svelte';

    function handleAddNode(typeId: string) {
        // Place new nodes near the center of the canvas.
        // A simple offset based on how many nodes exist.
        const count = brushGraph.nodeList.length;
        const x = 100 + (count % 4) * 180;
        const y = 50 + Math.floor(count / 4) * 120;
        brushGraph.addNode(typeId, x, y);
    }

    function handleReset() {
        brushGraph.resetToDefault();
    }
</script>

<div class="brush-builder">
    <div class="builder-toolbar">
        <span class="builder-title">Brush Graph</span>
        <NodePalette onaddnode={handleAddNode} />
        <button class="toolbar-btn" onclick={handleReset} title="Reset to default">Reset</button>
        <div class="spacer"></div>
        {#if brushGraph.error}
            <span class="error-badge" title={brushGraph.error}>Error</span>
        {/if}
        <button
            class="close-btn"
            onclick={() => brushGraph.isOpen = false}
            title="Close brush builder"
        >&times;</button>
    </div>

    <NodeCanvas />
</div>

<style>
    .brush-builder {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: #1e1e1e;
        border-top: 1px solid #333;
    }
    .builder-toolbar {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px;
        background: #252525;
        border-bottom: 1px solid #333;
        min-height: 28px;
    }
    .builder-title {
        font-size: 11px;
        font-weight: 600;
        color: #ccc;
    }
    .toolbar-btn {
        background: #3a3a3a;
        border: 1px solid #555;
        border-radius: 3px;
        color: #bbb;
        cursor: pointer;
        font-size: 10px;
        padding: 2px 8px;
    }
    .toolbar-btn:hover {
        background: #444;
    }
    .spacer {
        flex: 1;
    }
    .error-badge {
        font-size: 9px;
        color: #ff6b6b;
        background: #3a2020;
        padding: 2px 6px;
        border-radius: 3px;
        cursor: help;
    }
    .close-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        font-size: 16px;
        padding: 0 4px;
        line-height: 1;
    }
    .close-btn:hover {
        color: #ff6b6b;
    }
</style>
