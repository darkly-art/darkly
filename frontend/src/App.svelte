<script lang="ts">
    import LeftSidebar from './ui/LeftSidebar.svelte';
    import CanvasView from './canvas/CanvasView.svelte';
    import RightSidebar from './ui/RightSidebar.svelte';
    import BrushBuilder from './ui/brush_builder/BrushBuilder.svelte';
    import Toast from './ui/Toast.svelte';
    import PresetPicker from './ui/PresetPicker.svelte';
    import { brushGraph } from './state/brush_graph.svelte';
    // Register all tools
    import './tools/index';
    // Initialize hotkeys
    import './editor';
</script>

<div class="app-layout">
    <LeftSidebar />
    <div class="center-column">
        <CanvasView />
        {#if brushGraph.isOpen}
            <div class="brush-builder-panel">
                <BrushBuilder />
            </div>
        {/if}
    </div>
    <RightSidebar />
</div>
<Toast />
<PresetPicker />

<style>
    :global(body) {
        margin: 0;
        padding: 0;
        background: #1a1a1a;
        overflow: hidden;
        font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
        color: #e0e0e0;
        user-select: none;
    }

    .app-layout {
        display: flex;
        width: 100vw;
        height: 100vh;
        overflow: hidden;
    }

    .center-column {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-width: 0;
        overflow: hidden;
    }

    .brush-builder-panel {
        height: 280px;
        min-height: 200px;
        flex-shrink: 0;
    }
</style>
