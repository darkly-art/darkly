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

    /** Measure all node widgets in the DOM and run auto-layout with real sizes. */
    function handleAutoLayout() {
        const sizes: Record<string, [number, number]> = {};
        for (const el of document.querySelectorAll<HTMLElement>('[data-node-id]')) {
            const id = el.dataset.nodeId;
            if (id) sizes[id] = [el.offsetWidth, el.offsetHeight];
        }
        brushGraph.autoLayout(sizes);
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
        <span class="builder-title">Brush Builder</span>
        <NodePalette onaddnode={handleAddNode} />
        <button class="toolbar-btn" onclick={handleReset} title="Reset to default">Reset</button>
        <button class="toolbar-btn" onclick={handleAutoLayout} title="Auto-layout nodes">Layout</button>
        <div class="spacer"></div>
    </div>

    <div class="canvas-wrapper">
        <NodeCanvas />
        <button
            class="fullscreen-btn"
            onclick={() => fullscreen = !fullscreen}
            title={fullscreen ? "Exit fullscreen" : "Fullscreen"}
        >
            <i class={fullscreen ? 'fa-solid fa-compress' : 'fa-solid fa-expand'}></i>
        </button>
    </div>
</div>

<style>
    .brush-builder {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--bg);
    }
    .builder-toolbar {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px;
        background: var(--bg);
        border-bottom: 1px solid var(--bg-hover);
        min-height: 28px;
    }
    .builder-title {
        font-size: 11px;
        font-weight: 600;
        color: var(--text);
    }
    .toolbar-btn {
        background: var(--bg-hover);
        border: none;
        border-radius: 4px;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
        padding: 2px 8px;
        transition: background 0.1s, color 0.1s;
    }
    .toolbar-btn:hover {
        background: var(--bg-active);
        color: var(--text);
    }
    .spacer {
        flex: 1;
    }
    .canvas-wrapper {
        position: relative;
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
    }
    .fullscreen-btn {
        position: absolute;
        top: 8px;
        right: 8px;
        width: 28px;
        height: 28px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: color-mix(in srgb, var(--bg) 40%, transparent);
        border: none;
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        font-size: 12px;
        z-index: 10;
        transition: background 0.15s, color 0.15s;
    }
    .fullscreen-btn:hover {
        background: var(--accent);
        color: var(--text);
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
