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

    let fullscreen = $state(false);

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'Escape' && fullscreen) {
            fullscreen = false;
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="brush-builder" class:fullscreen>
    <div class="builder-toolbar">
        <span class="builder-title">Brush Graph</span>
        <NodePalette onaddnode={handleAddNode} />
        <button class="toolbar-btn" onclick={handleReset} title="Reset to default">Reset</button>
        <div class="spacer"></div>
        <button
            class="fullscreen-btn"
            onclick={() => fullscreen = !fullscreen}
            title={fullscreen ? "Exit fullscreen" : "Fullscreen"}
        >{#if fullscreen}
                <svg width="14" height="14" viewBox="0 0 14 14"><path d="M5 1H1v4M9 1h4v4M1 9v4h4M13 9v4h-4" stroke="currentColor" stroke-width="1.5" fill="none"/></svg>
            {:else}
                <svg width="14" height="14" viewBox="0 0 14 14"><path d="M1 5V1h4M13 5V1H9M1 9v4h4M13 9v4H9" stroke="currentColor" stroke-width="1.5" fill="none"/></svg>
            {/if}</button>
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
    .fullscreen-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        padding: 0 4px;
        display: flex;
        align-items: center;
    }
    .fullscreen-btn:hover {
        color: #ccc;
    }
    .brush-builder.fullscreen {
        position: fixed;
        top: 0;
        left: 0;
        width: 100vw;
        height: 100vh;
        z-index: 9999;
    }
</style>
